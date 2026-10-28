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

/// Arquivo sendo enviado.
pub struct Outgoing {
    pub id: u64,
    pub name: String,
    pub size: u64,
    file: File,
    sent: u64,
    acked: u64,
}

impl Outgoing {
    /// Abre um arquivo comum. Pastas, atalhos quebrados e arquivos grandes demais falham.
    pub fn open(id: u64, path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "não é um arquivo",
            ));
        }
        if meta.len() > MAX_FILE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "arquivo grande demais",
            ));
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.trim_end_matches(['.', ' ']).to_string())
            .filter(|n| protocol::is_valid_file_name(n))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "nome inválido"))?;
        Ok(Self {
            id,
            name,
            size: meta.len(),
            file,
            sent: 0,
            acked: 0,
        })
    }

    /// Próximo pedaço, se a janela de envio permitir. `Ok(None)`: aguardando confirmação
    /// ou tudo enviado (veja [`Outgoing::all_sent`]).
    pub fn next_chunk(&mut self) -> io::Result<Option<(u64, Vec<u8>)>> {
        if self.sent >= self.size || self.sent - self.acked >= WINDOW_BYTES {
            return Ok(None);
        }
        let want = (self.size - self.sent).min(MAX_FILE_CHUNK as u64) as usize;
        let mut buf = vec![0u8; want];
        self.file.read_exact(&mut buf)?;
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
