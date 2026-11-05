#!/usr/bin/env python3
"""Compares what the files in tests/data/svg come out as with what Chromium makes of them.

    python3 tools/svg_compare.py [scale]

Takes Chromium with GPU raster, and the `dump` test of tests/svg.rs. Prints for each file the mean of
the largest channel difference (0-255) over the pixels, and the largest difference.
"""
import os, re, subprocess, sys, tempfile, zlib, struct

HERE = os.path.dirname(os.path.abspath(__file__))
DIR = os.path.join(HERE, "..", "tests", "data", "svg")
W, H = 400, 300


def read_png(path):
    data = open(path, "rb").read()
    i = 8
    idat = b""
    while i < len(data):
        n = struct.unpack(">I", data[i : i + 4])[0]
        kind = data[i + 4 : i + 8]
        body = data[i + 8 : i + 8 + n]
        if kind == b"IHDR":
            w, h, depth, ctype = struct.unpack(">IIBB", body[:10])
        elif kind == b"IDAT":
            idat += body
        i += 12 + n
    raw = zlib.decompress(idat)
    channels = {2: 3, 6: 4}[ctype]
    stride = w * channels
    rows = []
    prev = bytearray(stride)
    p = 0
    for _ in range(h):
        f = raw[p]
        line = bytearray(raw[p + 1 : p + 1 + stride])
        p += 1 + stride
        for x in range(stride):
            a = line[x - channels] if x >= channels else 0
            b = prev[x]
            c = prev[x - channels] if x >= channels else 0
            if f == 1:
                line[x] = (line[x] + a) & 255
            elif f == 2:
                line[x] = (line[x] + b) & 255
            elif f == 3:
                line[x] = (line[x] + (a + b) // 2) & 255
            elif f == 4:
                pa, pb, pc = abs(b - c), abs(a - c), abs(a + b - 2 * c)
                line[x] = (line[x] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        rows.append(line)
        prev = line
    return w, h, channels, rows


def main():
    scale = float(sys.argv[1]) if len(sys.argv) > 1 else 1.0
    with tempfile.TemporaryDirectory() as tmp:
        env = dict(os.environ, SVG_DUMP=tmp, SVG_SCALE=str(scale))
        for key in ("CARGO_INCREMENTAL", "CARGO_PROFILE_TEST_DEBUG", "CARGO_PROFILE_DEV_DEBUG"):
            env[key] = "0"
        subprocess.run(
            ["cargo", "test", "-p", "anti-webui-app", "--test", "svg", "--", "--ignored", "dump", "--exact"],
            check=True,
            env=env,
            capture_output=True,
            cwd=os.path.join(HERE, ".."),
        )
        rows = []
        for name in sorted(os.listdir(DIR)):
            if not name.endswith(".svg"):
                continue
            stem = name[:-4]
            page = os.path.join(tmp, stem + ".html")
            svg = open(os.path.join(DIR, name)).read()
            open(page, "w").write(
                "<!doctype html><html><style>html,body{margin:0;background:#fff}svg{display:block}</style><body>"
                + svg
                + "</body></html>"
            )
            shot = os.path.join(tmp, stem + ".png")
            subprocess.run(
                [
                    "chromium", "--headless=new", "--no-sandbox", "--enable-gpu-rasterization",
                    "--use-angle=gl-egl", "--hide-scrollbars",
                    f"--force-device-scale-factor={scale}", f"--window-size={W},{H}",
                    f"--screenshot={shot}", "file://" + page,
                ],
                capture_output=True,
                timeout=60,
            )
            w, h, ch, png = read_png(shot)
            ours = open(os.path.join(tmp, stem + ".rgba"), "rb").read()
            pw = int(W * scale)
            total = 0
            worst = 0
            count = 0
            for y in range(min(h, int(H * scale))):
                line = png[y]
                for x in range(min(w, pw)):
                    i = (y * pw + x) * 4
                    d = max(abs(line[x * ch + c] - ours[i + c]) for c in range(3))
                    total += d
                    worst = max(worst, d)
                    count += 1
            rows.append((stem, total / count, worst))
    print(f"{'file':20} {'mean':>7} {'max':>4}")
    for stem, mean, worst in rows:
        print(f"{stem:20} {mean:7.3f} {worst:4}")
    print(f"{'all':20} {sum(r[1] for r in rows) / len(rows):7.3f}")


main()
