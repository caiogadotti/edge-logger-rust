use serde::{Deserialize, Serialize};

pub const TAM_LEITURA: usize = 18;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Leitura {
    pub sensor_id: u16,
    pub timestamp_us: u64,
    pub valor: f64,
}

impl Leitura {
    pub fn escrever(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.sensor_id.to_le_bytes());
        out.extend_from_slice(&self.timestamp_us.to_le_bytes());
        out.extend_from_slice(&self.valor.to_le_bytes());
    }

    pub fn ler(b: &[u8]) -> Leitura {
        Leitura {
            sensor_id: u16::from_le_bytes([b[0], b[1]]),
            timestamp_us: u64::from_le_bytes(b[2..10].try_into().unwrap()),
            valor: f64::from_le_bytes(b[10..18].try_into().unwrap()),
        }
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn ida_e_volta_crua() {
        let l = Leitura {
            sensor_id: 7,
            timestamp_us: 1_700_000_000_123_456,
            valor: -3.25,
        };
        let mut buf = Vec::new();
        l.escrever(&mut buf);
        assert_eq!(buf.len(), TAM_LEITURA);
        assert_eq!(Leitura::ler(&buf), l);
    }
}
