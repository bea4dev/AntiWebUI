#!/usr/bin/env python3
"""The diagram of examples/bench_svg.rs in Chromium, with GPU raster.

    python3 tools/svg_bench.py <boxes>

Prints how long putting it in the page, its layout, and the first frame took as the page sees them,
and the memory of all of Chromium's processes.
"""
import math, os, re, subprocess, sys, tempfile, time

boxes = int(sys.argv[1]) if len(sys.argv) > 1 else 300
columns = math.ceil(math.sqrt(boxes))
rows = math.ceil(boxes / columns)
full_w, full_h = columns * 180 + 40, rows * 110 + 40
W, H = 960, 898
fit = min(W / full_w, H / full_h, 1)
BW, BH = 140, 56


def place(i):
    return 20 + (i % columns) * 180, 20 + (i // columns) * 110


parts = [
    f'<svg xmlns="http://www.w3.org/2000/svg" width="{full_w * fit}" height="{full_h * fit}" '
    f'viewBox="0 0 {full_w} {full_h}" style="display:block"><g fill="none" stroke="#333" stroke-width="1.5">'
]
for i in range(1, boxes):
    ax, ay = place((i * 7919) % max(i, 1))
    bx, by = place(i)
    a = (ax + BW * 0.5, ay + BH)
    b = (bx + BW * 0.5, by)
    mid = (a[1] + b[1]) * 0.5
    parts.append(f'<path d="M{a[0]} {a[1]} C{a[0]} {mid} {b[0]} {mid} {b[0]} {b[1]}"/>')
parts.append("</g><g>")
for i in range(boxes):
    x, y = place(i)
    parts.append(
        f'<g><rect x="{x}" y="{y}" width="{BW}" height="{BH}" rx="8" fill="#dde8f8" stroke="#3366aa"/>'
        f'<text x="{x + BW * 0.5}" y="{y + BH * 0.5}" text-anchor="middle" dominant-baseline="middle" '
        f'font-size="14" fill="#222">Node number {i}</text></g>'
    )
parts.append("</g></svg>")
svg = "".join(parts)
page = f"""<!doctype html><html><style>html,body{{margin:0;background:#fff}}</style><body>
{svg}
<script>
new PerformanceObserver(list => {{
  for (const e of list.getEntries()) {{
    if (e.name === 'first-contentful-paint') {{
      const n = performance.getEntriesByType('navigation')[0];
      console.log(`RESULT dom ${{n.domInteractive.toFixed(1)}} ms, first paint with raster ${{e.startTime.toFixed(1)}} ms`);
    }}
  }}
}}).observe({{ type: 'paint', buffered: true }});
</script></body></html>"""
with tempfile.TemporaryDirectory() as tmp:
    file = os.path.join(tmp, "page.html")
    open(file, "w").write(page)
    # Stays open long enough for reading the memory from another process.
    p = subprocess.Popen(
        [
            "chromium", "--headless=new", "--no-sandbox", "--enable-gpu-rasterization", "--use-angle=gl-egl",
            "--force-device-scale-factor=1.8", f"--window-size={W},{H}", "--timeout=15000",
            f"--user-data-dir={tmp}/profile", "--enable-logging=stderr", f"--screenshot={tmp}/s.png", "file://" + file,
        ],
        stdout=subprocess.DEVNULL,
        stderr=open(tmp + "/log", "w"),
        text=True,
    )
    peak = 0
    start = time.time()
    while p.poll() is None and time.time() - start < 90:
        pids = subprocess.run(["pgrep", "-f", tmp], capture_output=True, text=True).stdout.split()
        total = 0
        for pid in pids:
            try:
                for line in open(f"/proc/{pid}/smaps_rollup"):
                    if line.startswith("Pss:"):
                        total += int(line.split()[1])
            except OSError:
                pass
        peak = max(peak, total)
        time.sleep(0.05)
    m = re.search(r'RESULT (.*?)"', open(tmp + "/log").read())
    print(f"boxes {boxes}: {m.group(1) if m else 'no result'}; peak PSS {peak // 1024} MiB")
