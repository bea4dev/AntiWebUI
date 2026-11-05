#!/usr/bin/env python3
"""Fills in what Chromium makes of the layout cases in a test data file.

    python3 tools/chrome_layout.py tests/data/flex_wrap.txt

A case is a `case` line, an optional `parent`, a `box` and any number of `item`s, which may hold one
child after ` > `. The `=` lines after it are rewritten: the rectangles of the box and of each item,
relative to the parent, as `x y width height`.
"""
import html, os, re, subprocess, sys, tempfile

DEFAULT_PARENT = "display:block;width:400px;height:300px"


def parse(path):
    cases = []
    for line in open(path):
        line = line.rstrip("\n")
        if line.startswith("case "):
            cases.append({"name": line[5:], "parent": DEFAULT_PARENT, "box": "", "items": []})
        elif line.startswith("parent "):
            cases[-1]["parent"] = line[7:]
        elif line.startswith("box "):
            cases[-1]["box"] = line[4:]
        elif line.startswith("item"):
            cases[-1]["items"].append(line[5:])
    return cases


def page(cases):
    out = ["<!doctype html><html><style>*{box-sizing:border-box}</style><body style='margin:0'>"]
    for i, c in enumerate(cases):
        out.append(f"<div id=c{i} style='{c['parent']}'><div style='{c['box']}'>")
        for item in c["items"]:
            own, _, child = item.partition(" > ")
            inner = f"<div style='flex:none;{child}'></div>" if child else ""
            out.append(f"<div style='{own}'>{inner}</div>")
        out.append("</div></div>")
    out.append(
        """<pre id=out></pre><script>
const lines = [];
for (const p of document.querySelectorAll('[id^=c]')) {
  const o = p.getBoundingClientRect();
  const box = p.firstElementChild;
  for (const e of [box, ...box.children]) {
    const r = e.getBoundingClientRect();
    lines.push([r.x - o.x, r.y - o.y, r.width, r.height].join(' '));
  }
  lines.push('');
}
document.getElementById('out').textContent = lines.join('\\n');
</script></body></html>"""
    )
    return "".join(out)


def main():
    path = sys.argv[1]
    cases = parse(path)
    with tempfile.TemporaryDirectory() as tmp:
        file = os.path.join(tmp, "cases.html")
        open(file, "w").write(page(cases))
        dom = subprocess.run(
            ["chromium", "--headless=new", "--no-sandbox", "--disable-gpu", "--dump-dom", "file://" + file],
            capture_output=True,
            text=True,
            timeout=60,
        ).stdout
    text = html.unescape(re.search(r'<pre id="out">(.*?)</pre>', dom, re.S).group(1))
    results = text.split("\n\n")
    assert len(results) >= len(cases), (len(results), len(cases))
    lines = [l for l in open(path) if not l.startswith("=")]
    out = []
    index = -1
    for j, line in enumerate(lines):
        out.append(line)
        last = j + 1 == len(lines) or lines[j + 1].startswith("case ") or not lines[j + 1].strip()
        if line.startswith("case "):
            index += 1
        if index >= 0 and last and line.strip():
            out.extend(f"= {r}\n" for r in results[index].strip().split("\n"))
    open(path, "w").writelines(out)


main()
