//! Write-ahead log: registros `[len u32][crc32 u32][payload]` com fsync em lote.
//! Na recuperação, tudo depois do primeiro registro inválido (energia caiu no meio) é truncado.

use crate::codec::crc;
use crate::model::{Leitura, TAM_LEITURA};
use anyhow::Result;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

const CABECALHO: usize = 8;
const MAX_REGISTRO: u32 = 64 * 1024 * 1024;

pub struct Wal {
    arquivo: File,
    pendente: Vec<u8>,
}

pub struct Recuperacao {
    pub leituras: Vec<Leitura>,
    pub bytes_truncados: u64,
}

impl Wal {
    /// Abre (ou cria) o WAL, recupera as leituras válidas e trunca o lixo do final.
    pub fn abrir(caminho: &Path) -> Result<(Wal, Recuperacao)> {
        let mut arquivo = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(caminho)?;
        let mut tudo = Vec::new();
        arquivo.read_to_end(&mut tudo)?;

        let (leituras, valido) = varrer(&tudo);
        let bytes_truncados = (tudo.len() - valido) as u64;
        if bytes_truncados > 0 {
            arquivo.set_len(valido as u64)?;
            arquivo.sync_all()?;
        }
        arquivo.seek(SeekFrom::End(0))?;
        Ok((
            Wal {
                arquivo,
                pendente: Vec::new(),
            },
            Recuperacao {
                leituras,
                bytes_truncados,
            },
        ))
    }

    pub fn adicionar(&mut self, l: &Leitura) {
        l.escrever(&mut self.pendente);
    }

    pub fn pendente_bytes(&self) -> usize {
        self.pendente.len()
    }

    /// Grava o lote pendente como um registro e faz fsync (RN-02).
    pub fn sincronizar(&mut self, fsync: bool) -> Result<()> {
        if self.pendente.is_empty() {
            return Ok(());
        }
        let mut reg = Vec::with_capacity(CABECALHO + self.pendente.len());
        reg.extend_from_slice(&(self.pendente.len() as u32).to_le_bytes());
        reg.extend_from_slice(&crc(&self.pendente).to_le_bytes());
        reg.extend_from_slice(&self.pendente);
        self.arquivo.write_all(&reg)?;
        if fsync {
            self.arquivo.sync_data()?;
        }
        self.pendente.clear();
        Ok(())
    }

    /// Chamado depois que o conteúdo virou bloco em segmento (já com fsync lá).
    pub fn zerar(&mut self) -> Result<()> {
        self.pendente.clear();
        self.arquivo.set_len(0)?;
        self.arquivo.seek(SeekFrom::Start(0))?;
        self.arquivo.sync_all()?;
        Ok(())
    }
}

/// Leitura sem efeito colateral (para `read`/`verify` enquanto o gravador roda).
pub fn varrer_publico(b: &[u8]) -> Vec<Leitura> {
    varrer(b).0
}

/// Devolve as leituras válidas e quantos bytes do início formam registros íntegros.
fn varrer(b: &[u8]) -> (Vec<Leitura>, usize) {
    let mut pos = 0;
    let mut out = Vec::new();
    while pos + CABECALHO <= b.len() {
        let len = u32::from_le_bytes(b[pos..pos + 4].try_into().unwrap());
        let soma = u32::from_le_bytes(b[pos + 4..pos + 8].try_into().unwrap());
        let ini = pos + CABECALHO;
        let fim = ini + len as usize;
        if len == 0
            || len > MAX_REGISTRO
            || !(len as usize).is_multiple_of(TAM_LEITURA)
            || fim > b.len()
        {
            break;
        }
        let payload = &b[ini..fim];
        if crc(payload) != soma {
            break;
        }
        out.extend(
            payload
                .as_chunks::<TAM_LEITURA>()
                .0
                .iter()
                .map(|c| Leitura::ler(c)),
        );
        pos = fim;
    }
    (out, pos)
}

#[cfg(test)]
mod testes {
    use super::*;

    fn leitura(i: u64) -> Leitura {
        Leitura {
            sensor_id: (i % 3) as u16,
            timestamp_us: i,
            valor: i as f64,
        }
    }

    #[test]
    fn grava_e_recupera() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("wal.log");
        {
            let (mut w, r) = Wal::abrir(&p).unwrap();
            assert!(r.leituras.is_empty());
            for i in 0..1000 {
                w.adicionar(&leitura(i));
                if i % 100 == 99 {
                    w.sincronizar(true).unwrap();
                }
            }
        }
        let (_, r) = Wal::abrir(&p).unwrap();
        assert_eq!(r.leituras.len(), 1000);
        assert_eq!(r.bytes_truncados, 0);
    }

    /// Simula a energia caindo em qualquer byte: nunca perde registro completo e nunca devolve lixo.
    #[test]
    fn truncamento_em_todo_byte() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("wal.log");
        {
            let (mut w, _) = Wal::abrir(&p).unwrap();
            for lote in 0..5u64 {
                for i in 0..10 {
                    w.adicionar(&leitura(lote * 10 + i));
                }
                w.sincronizar(true).unwrap();
            }
        }
        let completo = std::fs::read(&p).unwrap();
        let tam_reg = CABECALHO + 10 * TAM_LEITURA;
        for corte in 0..=completo.len() {
            std::fs::write(&p, &completo[..corte]).unwrap();
            let (_, r) = Wal::abrir(&p).unwrap();
            assert_eq!(r.leituras.len(), (corte / tam_reg) * 10, "corte em {corte}");
            assert_eq!(
                std::fs::metadata(&p).unwrap().len() as usize,
                (corte / tam_reg) * tam_reg
            );
        }
    }

    #[test]
    fn byte_corrompido_para_a_leitura() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("wal.log");
        {
            let (mut w, _) = Wal::abrir(&p).unwrap();
            for lote in 0..3u64 {
                w.adicionar(&leitura(lote));
                w.sincronizar(true).unwrap();
            }
        }
        let mut b = std::fs::read(&p).unwrap();
        let tam_reg = CABECALHO + TAM_LEITURA;
        b[tam_reg + CABECALHO + 3] ^= 0xff;
        std::fs::write(&p, &b).unwrap();
        let (_, r) = Wal::abrir(&p).unwrap();
        assert_eq!(r.leituras.len(), 1);
    }
}
