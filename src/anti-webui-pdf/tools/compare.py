#!/usr/bin/env python3
"""Compares `bench_pdf` with the viewer of PDF.js in Chromium, at the same size and scrolling speed.

    cargo build --release -p anti-webui-app --example bench_pdf
    src/anti-webui-pdf/tools/compare.py --fetch WORK_DIR
    src/anti-webui-pdf/tools/compare.py WORK_DIR [--zoom PERCENT] [more.pdf ...]
    src/anti-webui-pdf/tools/compare.py WORK_DIR --find WORD [more.pdf ...]

The samples and PDF.js go in WORK_DIR, which shouldn't be in the repository: the licenses of the
samples are their own. Chromium is only here to compare with. See `docs/design/26-pdf.md`.
"""
import http.server
import json
import os
import shutil
import signal
import statistics
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
import urllib.parse

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))
BENCH = os.path.join(REPO, "target/release/examples/bench_pdf")
BENCH_TEXT = os.path.join(REPO, "target/release/examples/bench_pdf_text")
PDFJS = "https://registry.npmjs.org/pdfjs-dist/-/pdfjs-dist-6.5.438.tgz"
SAMPLES = {
    "spec-pdf32000.pdf": "https://opensource.adobe.com/dc-acrobat-sdk-docs/pdfstandards/PDF32000_2008.pdf",
    "tracemonkey.pdf": "https://raw.githubusercontent.com/mozilla/pdf.js/master/test/pdfs/tracemonkey.pdf",
    "tube-map.pdf": "https://content.tfl.gov.uk/standard-tube-map.pdf",
    "scan-origin-of-species.pdf": "https://ia600501.us.archive.org/35/items/originofspecies00darwuoft/originofspecies00darwuoft.pdf",
    "slides-vulkan.pdf": "https://www.khronos.org/assets/uploads/developers/library/overview/vulkan-overview.pdf",
    "map-usgs.pdf": "https://pubs.usgs.gov/sim/3340/sim3340_sheet1.pdf",
}
# The size bench_pdf draws at: 2340x1616 at a scale of 1.8.
WINDOW = "1300,898"


def fetch(work):
    os.makedirs(os.path.join(work, "samples"), exist_ok=True)
    for name, url in SAMPLES.items():
        path = os.path.join(work, "samples", name)
        if not os.path.exists(path):
            subprocess.run(["curl", "-sSfL", "--max-time", "300", "-o", path, url], check=True)
    package = os.path.join(work, "package")
    if not os.path.exists(package):
        tgz = os.path.join(work, "pdfjs.tgz")
        subprocess.run(["curl", "-sSfL", "--max-time", "300", "-o", tgz, PDFJS], check=True)
        with tarfile.open(tgz) as t:
            t.extractall(work, filter="data")


def pss_of_tree(pid):
    total, seen = 0, [pid]
    for p in seen:
        try:
            seen += [int(k) for k in open(f"/proc/{p}/task/{p}/children").read().split()]
            for line in open(f"/proc/{p}/smaps_rollup"):
                if line.startswith("Pss:"):
                    total += int(line.split()[1])
        except OSError:
            pass
    return total


def serve(work, results, finished):
    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def do_GET(self):
            url = urllib.parse.urlparse(self.path)
            if url.path == "/file":
                path = urllib.parse.parse_qs(url.query)["p"][0]
            elif url.path == "/page.html":
                path = os.path.join(HERE, "pdfjs_page.html")
            elif url.path.startswith("/pdfjs/"):
                path = os.path.join(work, "package", url.path[7:])
            else:
                self.send_error(404)
                return
            if not os.path.isfile(path):
                self.send_error(404)
                return
            data = open(path, "rb").read()
            kind = {".mjs": "text/javascript", ".html": "text/html", ".css": "text/css",
                    ".wasm": "application/wasm"}.get(os.path.splitext(path)[1], "application/octet-stream")
            self.send_response(200)
            self.send_header("Content-Type", kind)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_POST(self):
            body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
            if self.path == "/result":
                results.append(json.loads(body))
            elif self.path == "/done":
                finished.set()
            self.send_response(200)
            self.send_header("Content-Length", "0")
            self.end_headers()

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def pdfjs(work, file, speed, seconds, zoom, find=""):
    results, finished = [], threading.Event()
    server = serve(work, results, finished)
    profile = tempfile.mkdtemp()
    query = urllib.parse.urlencode({"file": file, "speed": speed, "seconds": seconds,
                                    "zoom": zoom / 100 if zoom else 0, "find": find})
    chromium = subprocess.Popen(
        ["chromium", "--headless=new", "--no-sandbox", "--enable-gpu-rasterization",
         "--ignore-gpu-blocklist", "--use-angle=gl-egl", "--no-first-run",
         f"--window-size={WINDOW}", "--force-device-scale-factor=1.8",
         f"--user-data-dir={profile}",
         f"http://127.0.0.1:{server.server_address[1]}/page.html?{query}"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
    peak, end = 0, time.time() + seconds + 120
    while not finished.is_set() and time.time() < end:
        peak = max(peak, pss_of_tree(chromium.pid))
        time.sleep(0.1)
    os.killpg(chromium.pid, signal.SIGKILL)
    chromium.wait()
    server.shutdown()
    shutil.rmtree(profile, ignore_errors=True)
    result = results[0] if results else {"error": "timed out"}
    result["peak_mib"] = peak / 1024
    return result


def ours(file, speed, seconds, zoom):
    zoomed = ["--zoom", str(zoom)] if zoom else []
    out = subprocess.run([BENCH, file, "--speed", str(speed), "--seconds", str(seconds), *zoomed],
                         capture_output=True, text=True, timeout=seconds + 120).stdout
    return json.loads(out.splitlines()[-1])


def ours_find(file, word):
    out = subprocess.run([BENCH_TEXT, file, "--no-copy", "--search", word],
                         capture_output=True, text=True, timeout=300).stdout
    got = json.loads(out.splitlines()[0])
    return {"first_ms": got["first_hit_ms"], "all_ms": got["ms"],
            "longest_frame_ms": got["longest_frame_ms"], "hits": got["hits"]}


def compare_find(work, word, files):
    print(f"{'':30}{'':8}{'first':>9}{'all':>9}{'longest':>9}{'hits':>8}")
    for file in files:
        for name, run in (("hayro", lambda: ours_find(file, word)),
                          ("pdfjs", lambda: pdfjs(work, file, 1, 1, 0, word))):
            got = [run() for _ in range(2)]
            bad = [g for g in got if "error" in g]
            if bad:
                print(f"{os.path.basename(file)[:28]:30}{name:8}{bad[0]['error']}", flush=True)
                continue
            best = lambda key: min(g[key] for g in got)
            print(f"{os.path.basename(file)[:28]:30}{name:8}{best('first_ms'):9.0f}{best('all_ms'):9.0f}"
                  f"{best('longest_frame_ms'):9.1f}{got[0]['hits']:8}", flush=True)


def main():
    args = sys.argv[1:]
    if args and args[0] == "--fetch":
        fetch(args[1])
        return
    work, more, zoom = args[0], args[1:], 0
    if more[:1] == ["--zoom"]:
        zoom, more = int(more[1]), more[2:]
    word = None
    if more[:1] == ["--find"]:
        word, more = more[1], more[2:]
    samples = os.path.join(work, "samples")
    files = [os.path.join(samples, f) for f in sorted(os.listdir(samples))] + more
    if word:
        compare_find(work, word, files)
        return
    rounds = 2
    seconds = 5
    print(f"{'':30}{'':8}{'shown':>9}{'sharp':>9}{'sharp%':>8}{'opened':>9}{'blank':>8}{'p95':>8}"
          f"{'max':>8}{'peak':>8}  page")
    for file in files:
        for speed in (1, 5):
            for name, run in (("hayro", lambda: ours(file, speed, seconds, zoom)),
                              ("pdfjs", lambda: pdfjs(work, file, speed, seconds, zoom))):
                got = [run() for _ in range(rounds)]
                bad = [g for g in got if "error" in g]
                if bad:
                    print(f"{os.path.basename(file)[:28]:30}{name:8}{bad[0]['error']}", flush=True)
                    continue
                best = lambda key: min(g[key] for g in got)
                print(f"{os.path.basename(file)[:26] + ' x' + str(speed):30}{name:8}"
                      f"{best('shown_gpu_ms'):9.0f}{best('sharp_ms'):9.0f}"
                      f"{statistics.mean(g['sharp_share'] for g in got) * 100:7.1f}%"
                      f"{best('opened_ms'):9.0f}"
                      f"{statistics.mean(g['blank_screens_share'] for g in got) * 100:7.1f}%"
                      f"{best('frame_p95_ms'):8.1f}{best('frame_max_ms'):8.1f}"
                      f"{max(g['peak_mib'] for g in got):8.0f}  {got[0]['page_reached']}", flush=True)


if __name__ == "__main__":
    main()
