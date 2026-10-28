//! Transferência de arquivos entre os PCs (Fase 5).
//!
//! Só lógica de disco e de janela de envio; a rede fica no motor. O envio é sequencial,
//! com no máximo [`WINDOW_BYTES`] sem confirmação, para os chunks nunca atrasarem o
//! heartbeat nem a entrada do mouse que dividem o mesmo canal.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use protocol::{MAX_FILE_BYTES, MAX_FILE_CHUNK};

/// Bytes enviados e ainda não confirmados pelo receptor.
pub const WINDOW_BYTES: u64 = 16 * MAX_FILE_CHUNK as u64;

/// Aceita apenas caminhos relativos sem `..`, drive, UNC ou segmentos vazios.
pub fn is_safe_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with(['/', '\\'])
        && !path.contains(':')
        && !path.split(['/', '\\']).any(|c| c == ".." || c.is_empty())
}

const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Nomes de dispositivo do Windows (CON, NUL, COM1...) viram arquivo comum com um prefixo.
pub fn disk_name(name: &str) -> String {
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
        format!("_{name}")
    } else {
        name.to_string()
    }
}

/// Cria, sem sobrescrever nada, um arquivo vazio em `dir` com o nome `name` (ou
/// `nome (1).ext`, `nome (2).ext`...) e devolve o caminho reservado.
fn reserve_unique(dir: &Path, name: &str) -> io::Result<PathBuf> {
    let name = disk_name(name);
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name.as_str(), ""),
    };
    for n in 0..10_000u32 {
        let candidate = if n == 0 {
            dir.join(&name)
        } else {
            dir.join(format!("{stem} ({n}){ext}"))
        };
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(_) => return Ok(candidate),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "nomes esgotados",
    ))
}

/// Um arquivo a enviar, com o caminho relativo a partir do item solto na borda.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    /// `a.txt` (avulso) ou `Pasta/sub/a.txt`; sempre com `/`.
    pub rel: String,
    pub size: u64,
    /// Total de bytes do item raiz a que o arquivo pertence.
    pub root_bytes: u64,
    /// Último arquivo do item raiz.
    pub root_last: bool,
}

/// O que será enviado: arquivos em ordem, mais o que foi ignorado.
#[derive(Debug, Default)]
pub struct Plan {
    pub entries: Vec<Entry>,
    /// Itens pulados (atalhos/links, nomes inválidos, passou do limite).
    pub skipped: usize,
    /// Itens raiz que eram pastas sem nenhum arquivo.
    pub empty_folders: usize,
}

/// Atalhos, links simbólicos e junções não são seguidos (evita laços e vazar outras pastas).
fn is_link(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0 // FILE_ATTRIBUTE_REPARSE_POINT
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

fn clean_name(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.trim_end_matches(['.', ' ']).to_string())
        .filter(|n| protocol::is_valid_file_name(n))
}

/// Lista, em ordem estável, os arquivos de `paths`: arquivos avulsos e, para cada pasta, todos
/// os arquivos dentro dela (recursivamente). Pastas vazias não são recriadas no destino.
pub fn plan(paths: &[PathBuf]) -> Plan {
    let mut plan = Plan::default();
    let mut total: u64 = 0;
    for root in paths {
        let Ok(meta) = std::fs::symlink_metadata(root) else {
            plan.skipped += 1;
            continue;
        };
        let Some(root_name) = clean_name(root) else {
            plan.skipped += 1;
            continue;
        };
        if is_link(&meta) {
            plan.skipped += 1;
            continue;
        }
        let first = plan.entries.len();
        if meta.is_file() {
            if meta.len() > MAX_FILE_BYTES {
                plan.skipped += 1;
                continue;
            }
            plan.entries.push(Entry {
                path: root.clone(),
                name: root_name.clone(),
                rel: root_name,
                size: meta.len(),
                root_bytes: 0,
                root_last: true,
            });
        } else if meta.is_dir() {
            walk(root, &root_name, 1, &mut plan, &mut total);
            if plan.entries.len() == first {
                plan.empty_folders += 1;
            }
        } else {
            plan.skipped += 1;
        }
        // Totais do item raiz: soma dos arquivos e marca do último.
        let bytes: u64 = plan.entries[first..].iter().map(|e| e.size).sum();
        for e in &mut plan.entries[first..] {
            e.root_bytes = bytes;
        }
        if let Some(last) = plan.entries[first..].last_mut() {
            last.root_last = true;
        }
        total = total.saturating_add(bytes);
    }
    plan
}

fn walk(dir: &Path, rel: &str, depth: usize, plan: &mut Plan, total: &mut u64) {
    if depth >= protocol::MAX_REL_DEPTH {
        plan.skipped += 1;
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        plan.skipped += 1;
        return;
    };
    let mut items: Vec<_> = rd.filter_map(Result::ok).collect();
    items.sort_by_key(|e| e.file_name());
    for item in items {
        let path = item.path();
        let (Ok(meta), Some(name)) = (std::fs::symlink_metadata(&path), clean_name(&path)) else {
            plan.skipped += 1;
            continue;
        };
        if is_link(&meta) {
            plan.skipped += 1;
        } else if meta.is_dir() {
            walk(&path, &format!("{rel}/{name}"), depth + 1, plan, total);
        } else if meta.is_file() {
            let over = meta.len() > MAX_FILE_BYTES
                || plan.entries.len() >= protocol::MAX_FILES_PER_BATCH as usize
                || total.saturating_add(meta.len()) > protocol::MAX_BATCH_BYTES;
            if over {
                plan.skipped += 1;
                continue;
            }
            *total += meta.len();
            plan.entries.push(Entry {
                path,
                name: name.clone(),
                rel: format!("{rel}/{name}"),
                size: meta.len(),
                root_bytes: 0,
                root_last: false,
            });
        } else {
            plan.skipped += 1;
        }
    }
}

/// Cria, sem sobrescrever nada, uma pasta `name` (ou `nome (1)`, `nome (2)`...) em `parent`.
pub fn create_unique_dir(parent: &Path, name: &str) -> io::Result<PathBuf> {
    std::fs::create_dir_all(parent)?;
    let name = disk_name(name);
    for n in 0..10_000u32 {
        let candidate = if n == 0 {
            parent.join(&name)
        } else {
            parent.join(format!("{name} ({n})"))
        };
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "nomes esgotados",
    ))
}

/// Arquivo sendo enviado (aberto só quando chega a vez dele).
pub struct Outgoing {
    pub id: u64,
    pub name: String,
    pub rel: String,
    pub size: u64,
    pub root_bytes: u64,
    pub root_last: bool,
    path: PathBuf,
    file: Option<File>,
    sent: u64,
    acked: u64,
}

impl Outgoing {
    pub fn from_entry(id: u64, e: &Entry) -> Self {
        Self {
            id,
            name: e.name.clone(),
            rel: e.rel.clone(),
            size: e.size,
            root_bytes: e.root_bytes,
            root_last: e.root_last,
            path: e.path.clone(),
            file: None,
            sent: 0,
            acked: 0,
        }
    }

    /// Atalho para um arquivo avulso. Pastas e arquivos grandes demais falham.
    pub fn open(id: u64, path: &Path) -> io::Result<Self> {
        let p = plan(&[path.to_path_buf()]);
        match p.entries.as_slice() {
            [e] if e.rel == e.name => Ok(Self::from_entry(id, e)),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "não é um arquivo",
            )),
        }
    }

    /// Próximo pedaço, se a janela de envio permitir. `Ok(None)`: aguardando confirmação
    /// ou tudo enviado (veja [`Outgoing::all_sent`]).
    pub fn next_chunk(&mut self) -> io::Result<Option<(u64, Vec<u8>)>> {
        if self.sent >= self.size || self.sent - self.acked >= WINDOW_BYTES {
            return Ok(None);
        }
        if self.file.is_none() {
            let f = File::open(&self.path)?;
            if f.metadata()?.len() != self.size {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "o arquivo mudou durante o envio",
                ));
            }
            self.file = Some(f);
        }
        let want = (self.size - self.sent).min(MAX_FILE_CHUNK as u64) as usize;
        let mut buf = vec![0u8; want];
        if let Some(f) = self.file.as_mut() {
            f.read_exact(&mut buf)?;
        }
        let offset = self.sent;
        self.sent += want as u64;
        Ok(Some((offset, buf)))
    }

    /// Registra a confirmação do receptor. Ignora valores que regridem ou passam do enviado.
    pub fn ack(&mut self, offset: u64) {
        if offset > self.acked && offset <= self.sent {
            self.acked = offset;
        }
    }

    pub fn all_sent(&self) -> bool {
        self.sent >= self.size
    }

    pub fn is_done(&self) -> bool {
        self.acked >= self.size
    }

    /// Bytes já confirmados pelo receptor.
    pub fn acked(&self) -> u64 {
        self.acked
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum WriteError {
    /// Offset fora de ordem ou além do tamanho anunciado.
    OutOfOrder,
    Io(io::ErrorKind),
}

/// Arquivo sendo recebido: grava em `<nome>.wfpart` e só renomeia ao completar.
pub struct Incoming {
    pub id: u64,
    pub name: String,
    pub size: u64,
    final_path: PathBuf,
    part_path: PathBuf,
    file: Option<File>,
    received: u64,
}

impl Incoming {
    pub fn create(dir: &Path, id: u64, name: &str, size: u64) -> io::Result<Self> {
        if size > MAX_FILE_BYTES || !protocol::is_valid_file_name(name) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "oferta inválida",
            ));
        }
        std::fs::create_dir_all(dir)?;
        let final_path = reserve_unique(dir, name)?;
        let mut part = final_path.clone().into_os_string();
        part.push(".wfpart");
        let part_path = PathBuf::from(part);
        let file = match OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&part_path)
        {
            Ok(f) => f,
            Err(e) => {
                let _ = std::fs::remove_file(&final_path);
                return Err(e);
            }
        };
        Ok(Self {
            id,
            name: final_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(name)
                .to_string(),
            size,
            final_path,
            part_path,
            file: Some(file),
            received: 0,
        })
    }

    pub fn received(&self) -> u64 {
        self.received
    }

    pub fn path(&self) -> &Path {
        &self.final_path
    }

    /// Grava o próximo pedaço (sempre em sequência). Devolve o total recebido.
    pub fn write(&mut self, offset: u64, data: &[u8]) -> Result<u64, WriteError> {
        let end = self.received.checked_add(data.len() as u64);
        if offset != self.received || end.is_none_or(|e| e > self.size) {
            return Err(WriteError::OutOfOrder);
        }
        let file = self.file.as_mut().ok_or(WriteError::OutOfOrder)?;
        file.seek(SeekFrom::Start(offset))
            .and_then(|_| file.write_all(data))
            .map_err(|e| WriteError::Io(e.kind()))?;
        self.received += data.len() as u64;
        Ok(self.received)
    }

    pub fn is_complete(&self) -> bool {
        self.received == self.size
    }

    /// Fecha, grava no disco e entrega o arquivo no nome final.
    pub fn finish(mut self) -> io::Result<PathBuf> {
        if !self.is_complete() {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "incompleto"));
        }
        if let Some(f) = self.file.take() {
            f.sync_all()?;
        }
        std::fs::rename(&self.part_path, &self.final_path)?;
        let path = self.final_path.clone();
        // O arquivo já está no lugar; impede o Drop de apagá-lo.
        self.final_path = PathBuf::new();
        self.part_path = PathBuf::new();
        Ok(path)
    }
}

impl Drop for Incoming {
    /// Recebimento interrompido: não deixa lixo nem arquivo pela metade em Downloads.
    fn drop(&mut self) {
        if self.part_path.as_os_str().is_empty() {
            return;
        }
        self.file = None;
        let _ = std::fs::remove_file(&self.part_path);
        let _ = std::fs::remove_file(&self.final_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("wf-ft-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn blocks_traversal_and_absolute() {
        for p in [
            "../x",
            "a/../b",
            "/etc/x",
            "C:\\x",
            "\\\\srv\\s",
            "a//b",
            "",
        ] {
            assert!(!is_safe_relative(p), "{p}");
        }
        assert!(is_safe_relative("pasta/ação.txt"));
    }

    #[test]
    fn device_names_are_neutralised() {
        assert_eq!(disk_name("CON"), "_CON");
        assert_eq!(disk_name("nul.txt"), "_nul.txt");
        assert_eq!(disk_name("console.txt"), "console.txt");
    }

    #[test]
    fn transfers_a_file_with_window_and_unique_names() {
        let dir = temp_dir("roundtrip");
        let src = dir.join("origem.bin");
        let data: Vec<u8> = (0..(MAX_FILE_CHUNK * 40 + 123))
            .map(|i| (i % 251) as u8)
            .collect();
        std::fs::write(&src, &data).unwrap();
        let dest = dir.join("downloads");

        for round in 0..2 {
            let mut out = Outgoing::open(7, &src).unwrap();
            let mut inc = Incoming::create(&dest, 7, &out.name, out.size).unwrap();
            let mut max_in_flight = 0;
            while !out.is_done() {
                while let Some((off, chunk)) = out.next_chunk().unwrap() {
                    inc.write(off, &chunk).unwrap();
                    max_in_flight = max_in_flight.max(off + chunk.len() as u64 - out.acked());
                }
                out.ack(inc.received());
            }
            assert!(max_in_flight <= WINDOW_BYTES, "janela estourada");
            assert!(inc.is_complete());
            let path = inc.finish().unwrap();
            let expected = if round == 0 {
                "origem.bin"
            } else {
                "origem (1).bin"
            };
            assert_eq!(path.file_name().unwrap(), expected);
            assert_eq!(std::fs::read(&path).unwrap(), data);
        }
        assert!(!dest.join("origem.bin.wfpart").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_out_of_order_and_oversized_writes() {
        let dir = temp_dir("order");
        let mut inc = Incoming::create(&dir, 1, "a.txt", 4).unwrap();
        assert_eq!(inc.write(2, b"xx"), Err(WriteError::OutOfOrder));
        assert_eq!(inc.write(0, b"12345"), Err(WriteError::OutOfOrder));
        assert_eq!(inc.write(0, b"1234"), Ok(4));
        assert!(inc.finish().is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn interrupted_receive_leaves_nothing_behind() {
        let dir = temp_dir("abort");
        let mut inc = Incoming::create(&dir, 1, "b.txt", 10).unwrap();
        inc.write(0, b"abc").unwrap();
        assert!(dir.join("b.txt").exists());
        drop(inc);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plans_a_folder_tree_with_relative_paths_and_root_totals() {
        let dir = temp_dir("plan");
        let root = dir.join("Projeto");
        std::fs::create_dir_all(root.join("src/sub")).unwrap();
        std::fs::create_dir_all(root.join("vazia")).unwrap();
        std::fs::write(root.join("a.txt"), b"12345").unwrap();
        std::fs::write(root.join("src/b.rs"), b"123").unwrap();
        std::fs::write(root.join("src/sub/c.md"), b"1").unwrap();
        let loose = dir.join("solto.bin");
        std::fs::write(&loose, b"xx").unwrap();
        let empty = dir.join("so-pasta");
        std::fs::create_dir_all(empty.join("dentro")).unwrap();

        let p = plan(&[root.clone(), loose, empty]);
        let rels: Vec<_> = p.entries.iter().map(|e| e.rel.as_str()).collect();
        assert_eq!(
            rels,
            [
                "Projeto/a.txt",
                "Projeto/src/b.rs",
                "Projeto/src/sub/c.md",
                "solto.bin"
            ]
        );
        assert_eq!(p.empty_folders, 1);
        let last: Vec<_> = p.entries.iter().map(|e| e.root_last).collect();
        assert_eq!(last, [false, false, true, true]);
        assert!(p.entries[..3].iter().all(|e| e.root_bytes == 9));
        assert_eq!(p.entries[3].root_bytes, 2);
        assert!(p.entries.iter().all(|e| protocol::is_valid_rel(&e.rel)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unique_dirs_never_overwrite() {
        let dir = temp_dir("udir");
        let a = create_unique_dir(&dir, "Pasta").unwrap();
        let b = create_unique_dir(&dir, "Pasta").unwrap();
        assert_eq!(a.file_name().unwrap(), "Pasta");
        assert_eq!(b.file_name().unwrap(), "Pasta (1)");
        assert_eq!(
            create_unique_dir(&dir, "CON").unwrap().file_name().unwrap(),
            "_CON"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_folders_and_bad_acks_are_ignored() {
        let dir = temp_dir("dir");
        assert!(Outgoing::open(1, &dir).is_err());
        let f = dir.join("c.txt");
        std::fs::write(&f, b"hello").unwrap();
        let mut out = Outgoing::open(1, &f).unwrap();
        out.ack(99);
        assert_eq!(out.acked(), 0);
        let (off, c) = out.next_chunk().unwrap().unwrap();
        assert_eq!((off, c.len()), (0, 5));
        assert!(out.all_sent() && !out.is_done());
        out.ack(5);
        assert!(out.is_done());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
