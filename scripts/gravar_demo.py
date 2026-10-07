"""Gera docs/demo.gif: roda os comandos de verdade, captura a saída e desenha um terminal quadro a quadro.

Roteiro: grava do simulador → "queda de energia" (processo morto) → religa e recupera → verify → bench.
"""
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

RAIZ = Path(__file__).resolve().parent.parent
BIN = RAIZ / "target" / "release" / "edgelog.exe"
FONTE = ImageFont.truetype("consola.ttf", 17)
L, A, MARGEM, ALT_LINHA, MAX_LINHAS = 1060, 520, 18, 22, 21
FUNDO, TEXTO, PROMPT, DESTAQUE, ALERTA = (24, 24, 27), (220, 220, 220), (120, 200, 120), (232, 140, 90), (230, 90, 90)


def rodar(args, segundos=None, dados=None):
    cmd = [str(BIN), *args]
    if segundos is None:
        r = subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", cwd=dados)
        return (r.stdout + r.stderr).strip().splitlines()
    p = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, encoding="utf-8", cwd=dados)
    time.sleep(segundos)
    p.kill()  # equivalente a puxar o cabo
    out, _ = p.communicate()
    return out.strip().splitlines()


def cor(linha):
    if linha.startswith("$"):
        return PROMPT
    if linha.startswith("#"):
        return DESTAQUE
    if "recuperação" in linha or "KILL" in linha:
        return ALERTA
    return TEXTO


class Terminal:
    def __init__(self):
        self.linhas: list[str] = []
        self.quadros: list[Image.Image] = []

    def quadro(self, repetir=1):
        img = Image.new("RGB", (L, A), FUNDO)
        d = ImageDraw.Draw(img)
        for i, (r, g, b) in enumerate([(237, 106, 94), (245, 191, 79), (98, 197, 84)]):
            d.ellipse((MARGEM + i * 22, 12, MARGEM + i * 22 + 12, 24), fill=(r, g, b))
        d.text((L // 2, 18), "edgelog", fill=(150, 150, 150), font=FONTE, anchor="mm")
        for i, linha in enumerate(self.linhas[-MAX_LINHAS:]):
            d.text((MARGEM, 40 + i * ALT_LINHA), linha[:100], fill=cor(linha), font=FONTE)
        self.quadros.extend([img] * repetir)

    def digitar(self, cmd):
        self.linhas.append("$ ")
        for i in range(0, len(cmd) + 1, 3):
            self.linhas[-1] = "$ " + cmd[:i]
            self.quadro()
        self.linhas[-1] = "$ " + cmd
        self.quadro(3)

    def mostrar(self, saida, pausa=1):
        for linha in saida:
            self.linhas.append(linha)
            self.quadro(pausa)

    def comentario(self, txt):
        self.linhas.append(f"# {txt}")
        self.quadro(6)


def main():
    tmp = Path(tempfile.mkdtemp())
    t = Terminal()
    t.comentario("grava 200 mil leituras/s de 8 sensores simulados")
    t.digitar("edgelog record --source sim --rate 200000 --status-s 1")
    saida = rodar(["record", "--source", "sim", "--rate", "200000", "--status-s", "1", "--dir", "data"], segundos=3.3, dados=tmp)
    t.mostrar([s for s in saida if s.startswith("[edgelog]")], pausa=5)
    t.linhas.append("^ KILL  (energia caiu no meio da gravação)")
    t.quadro(10)

    t.comentario("religa: o WAL é conferido e o pedaço incompleto é cortado")
    t.digitar("edgelog record --source sim --rate 200000 --count 100000")
    t.mostrar(rodar(["record", "--source", "sim", "--rate", "200000", "--count", "100000", "--status-s", "60", "--dir", "data"], dados=tmp), pausa=6)

    t.digitar("edgelog verify")
    t.mostrar(rodar(["verify", "--dir", "data"], dados=tmp), pausa=4)
    t.quadro(10)

    t.comentario("benchmark: 5 milhões de leituras, fsync ligado")
    t.digitar("edgelog bench --count 5000000")
    t.mostrar(rodar(["bench", "--count", "5000000"]), pausa=4)
    t.quadro(25)

    frames = tmp / "frames"
    frames.mkdir()
    for i, q in enumerate(t.quadros):
        q.save(frames / f"{i:04d}.png")
    saida_gif = RAIZ / "docs" / "demo.gif"
    subprocess.run([
        "ffmpeg", "-y", "-loglevel", "error", "-framerate", "10", "-i", str(frames / "%04d.png"),
        "-vf", "split[a][b];[a]palettegen=max_colors=32:stats_mode=diff[p];[b][p]paletteuse=dither=none:diff_mode=rectangle",
        "-loop", "0", str(saida_gif),
    ], check=True)
    shutil.rmtree(tmp, ignore_errors=True)
    print(f"{len(t.quadros)} quadros -> {saida_gif} ({saida_gif.stat().st_size / 1e6:.2f} MB)")


if __name__ == "__main__":
    main()
