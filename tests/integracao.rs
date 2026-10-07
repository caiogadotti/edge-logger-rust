use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

const BIN: &str = env!("CARGO_BIN_EXE_edgelog");

fn edgelog(args: &[&str]) -> std::process::Output {
    Command::new(BIN)
        .args(args)
        .output()
        .expect("rodar edgelog")
}

/// CA-04: 1 milhão de leituras entram e saem idênticas.
#[test]
fn ida_e_volta_um_milhao() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().to_str().unwrap();
    let n = 1_000_000u64;

    let mut entrada = String::with_capacity(n as usize * 50);
    for i in 0..n {
        let valor = ((i as f64) * 0.001).sin() * 100.0;
        entrada.push_str(&format!(
            "{{\"sensor_id\":{},\"timestamp_us\":{},\"valor\":{}}}\n",
            i % 16,
            1_000_000 + i * 7,
            valor
        ));
    }

    let mut rec = Command::new(BIN)
        .args([
            "record",
            "--source",
            "stdin",
            "--dir",
            d,
            "--no-fsync",
            "--status-s",
            "3600",
        ])
        .stdin(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    rec.stdin
        .take()
        .unwrap()
        .write_all(entrada.as_bytes())
        .unwrap();
    assert!(rec.wait().unwrap().success());

    let out = edgelog(&["read", "--dir", d, "--format", "csv"]);
    assert!(out.status.success());
    let csv = String::from_utf8(out.stdout).unwrap();
    let linhas: Vec<&str> = csv.lines().skip(1).collect();
    assert_eq!(linhas.len() as u64, n);

    let mut esperado: Vec<(u64, u16, String)> = (0..n)
        .map(|i| {
            (
                1_000_000 + i * 7,
                (i % 16) as u16,
                format!("{}", ((i as f64) * 0.001).sin() * 100.0),
            )
        })
        .collect();
    esperado.sort();
    for (linha, (ts, s, v)) in linhas.iter().zip(&esperado) {
        assert_eq!(*linha, format!("{s},{ts},{v}"));
    }

    let ver = edgelog(&["verify", "--dir", d]);
    assert!(
        ver.status.success(),
        "{}",
        String::from_utf8_lossy(&ver.stdout)
    );
}

/// CA-03: matar o processo no meio da gravação não corrompe nada e perde no máximo o lote em curso.
#[test]
fn queda_de_energia() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().to_str().unwrap();

    for rodada in 0..3 {
        let mut rec = Command::new(BIN)
            .args([
                "record",
                "--source",
                "sim",
                "--rate",
                "200000",
                "--dir",
                d,
                "--block",
                "20000",
                "--fsync-ms",
                "50",
                "--status-s",
                "3600",
            ])
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(700 + rodada * 300));
        rec.kill().unwrap(); // equivalente a kill -9 / TerminateProcess
        rec.wait().unwrap();
    }

    let ver = edgelog(&["verify", "--dir", d]);
    let txt = String::from_utf8_lossy(&ver.stdout);
    assert!(ver.status.success(), "verify falhou:\n{txt}");
    assert!(txt.contains(" 0 corrompidos"), "{txt}");

    // reabrir e gravar de novo depois das quedas continua funcionando
    let ok = Command::new(BIN)
        .args([
            "record",
            "--source",
            "sim",
            "--rate",
            "100000",
            "--count",
            "50000",
            "--dir",
            d,
            "--status-s",
            "3600",
        ])
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(ok.success());
    let out = edgelog(&["read", "--dir", d, "--format", "csv"]);
    assert!(String::from_utf8_lossy(&out.stdout).lines().count() > 50_000);
}

/// CA-05: precisa de um broker em localhost:1883 (ex.: `docker run -p 1883:1883 eclipse-mosquitto:2 mosquitto -c /mosquitto-no-auth.conf`).
#[test]
#[ignore]
fn entrada_mqtt() {
    use rumqttc::{Client, MqttOptions, QoS};
    assert!(
        std::net::TcpStream::connect_timeout(&"127.0.0.1:1883".parse().unwrap(), Duration::from_secs(2)).is_ok(),
        "sem broker MQTT em localhost:1883"
    );
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().to_str().unwrap().to_string();

    let mut rec = Command::new(BIN)
        .args([
            "record",
            "--source",
            "mqtt",
            "--mqtt-topic",
            "teste/edgelog",
            "--dir",
            &d,
            "--status-s",
            "3600",
        ])
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_secs(2));

    let (cliente, mut conexao) = Client::new(MqttOptions::new("pub-teste", "localhost", 1883), 100);
    let pub_thread = std::thread::spawn(move || {
        for lote in 0..100 {
            let payload: Vec<String> = (0..100)
                .map(|i| {
                    format!(
                        "{{\"sensor_id\":{},\"timestamp_us\":{},\"valor\":{}}}",
                        i % 4,
                        1_000 + lote * 100 + i,
                        i
                    )
                })
                .collect();
            cliente
                .publish(
                    "teste/edgelog",
                    QoS::AtLeastOnce,
                    false,
                    format!("[{}]", payload.join(",")),
                )
                .unwrap();
        }
        std::thread::sleep(Duration::from_secs(2));
        cliente.disconnect().ok();
    });
    for ev in conexao.iter() {
        if ev.is_err() {
            break;
        }
    }
    pub_thread.join().unwrap();
    std::thread::sleep(Duration::from_secs(1));
    rec.kill().unwrap();
    rec.wait().unwrap();

    let out = edgelog(&["read", "--dir", &d, "--format", "csv"]);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).lines().count() - 1,
        10_000
    );
}
