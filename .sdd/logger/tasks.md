# Tasks: Edge Logger em Rust

- [x] T01 `cargo new edgelog`, Cargo.toml com as crates, clap com os 5 subcomandos vazios, .gitignore
- [x] T02 `model.rs`: struct `Leitura`, encode/decode cru de 18 bytes + teste
- [x] T03 `codec.rs`: bloco colunar (delta-of-delta ts + XOR valores) + zstd + CRC; testes de ida e volta e de bit trocado
- [x] T04 `wal.rs`: append com registro len+crc, fsync em lote, `recover()` que trunca no último válido; teste truncando em cada byte
- [x] T05 `segment.rs`: writer (header, blocos, footer/índice), reader (iterar blocos, pular corrompidos), reparo de segmento sem footer
- [x] T06 `pipeline.rs`: thread de fonte → canal limitado → gravador; métricas atômicas; `--drop-on-full`
- [x] T07 `source/sim.rs`: senoide + ruído gaussiano + picos de falha; taxa alvo com controle de tempo
- [x] T08 `source/stdin.rs` (JSON por linha) e `source/mqtt.rs` (rumqttc)
- [x] T09 `record`: config TOML + flags, recuperação na partida, status periódico no terminal
- [x] T10 `read`: filtro por sensor/intervalo usando o índice; export CSV/JSON
- [x] T11 `verify` e `bench` (vazão, MB/s, taxa de compressão vs CSV)
- [x] T12 testes de integração: ida e volta 1M (CA-04), kill -9 (CA-03), MQTT com mosquitto (CA-05)
- [x] T13 gráfico do benchmark (`scripts/grafico_bench.py` → `docs/bench.png` e `bench-en.png`)
- [x] T14 CI (fmt, clippy, test em ubuntu+windows) e workflow de release (3 alvos)
- [x] T15 README.md (PT) + README.en.md: problema, arquitetura (diagrama), formato, números, GIF do terminal, "Como rodar" — feito, com camada "em linguagem simples" em cada seção; GIF do terminal ainda pendente
- [ ] T16 publicar no GitHub (conferir identidade do git), adicionar em "Projetos em Destaque" no perfil, atualizar backlog
