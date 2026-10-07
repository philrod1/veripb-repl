#!/usr/bin/env python3
"""Runs veripb-repl's TUI in a pseudo-terminal from a script of keystrokes
and writes screen snapshots as SVG (plus a .txt dump of the screen text).

Usage, from the repository root (needs a built veripb-repl and
VERIPB_REPL_VERIPB_BIN set):

    python3 demos/tools/tuishot.py demos/tools/<demo>.json [--text]

--text also writes each snapshot's screen text to a .txt file next to it.

Script format:
    {"cmd": ["target/debug/veripb-repl", "<args>"],   # relative to repo root
     "copy": ["demos/files"],      # copied into a temp dir the TUI runs in
     "cols": 110, "rows": 40,
     "steps": [{"keys": "..."}, {"wait": 0.5}, {"snap": "demos/img/x.svg"},
               {"click": ["─ Output", "▭"]}]}
Keys use JSON escapes: "\\r" Enter, "\\u001b" Esc. A "keys" step waits
until output has been idle for "settle" seconds (default 0.8). A "click"
step left-clicks the first occurrence of its second string on the first
screen row containing its first string (e.g. a pane's zoom button).
"""
import codecs, fcntl, html, json, os, pty, re, select, shutil, struct, sys, tempfile, termios, time

# Box-drawing characters drawn as SVG lines rather than font glyphs (many
# fonts lack some of them): arms out of the cell centre, and stroke weight.
BOX = {
    "─": ("lr", 1), "│": ("ud", 1), "━": ("lr", 2), "┃": ("ud", 2),
    "┌": ("rd", 1), "┐": ("ld", 1), "└": ("ur", 1), "┘": ("ul", 1),
    "├": ("udr", 1), "┤": ("udl", 1), "┬": ("lrd", 1), "┴": ("lru", 1),
    "┼": ("lrud", 1),
}
# Other symbols drawn as shapes: the pane headers' zoom buttons and the
# Proof pane's breakpoint marker.
SHAPES = {"▭", "⛶", "●"}


def box_segments(c, x, y, cw, ch, color):
    """Line segments drawing box character `c` in the cell at pixel (x, y),
    as (horizontal?, fixed coordinate, start, end, color, width) tuples."""
    arms, weight = BOX[c]
    width = 1.2 if weight == 1 else 3.0
    cx, cy = x + cw / 2, y + ch / 2
    span = {"l": (True, cy, x, cx), "r": (True, cy, cx, x + cw),
            "u": (False, cx, y, cy), "d": (False, cx, cy, y + ch)}
    return [span[a] + (color, width) for a in arms]


def merged_lines(segments):
    """SVG <line>s for `segments`, joining collinear touching ones."""
    out = []
    groups = {}
    for horiz, fixed, a, b, color, width in segments:
        groups.setdefault((horiz, round(fixed, 1), color, width), []).append((a, b))
    for (horiz, fixed, color, width), spans in groups.items():
        spans.sort()
        merged = [list(spans[0])]
        for a, b in spans[1:]:
            if a <= merged[-1][1] + 0.01:
                merged[-1][1] = max(merged[-1][1], b)
            else:
                merged.append([a, b])
        for a, b in merged:
            x1, y1, x2, y2 = (a, fixed, b, fixed) if horiz else (fixed, a, fixed, b)
            out.append(f'<line x1="{x1:.1f}" y1="{y1:.1f}" x2="{x2:.1f}" y2="{y2:.1f}" '
                       f'stroke="{color}" stroke-width="{width}"/>')
    return out


def glyph_svg(c, x, y, cw, ch, color):
    """SVG elements drawing shape `c` (one of SHAPES) in the cell at (x, y)."""
    cx, cy = x + cw / 2, y + ch / 2
    if c == "▭":
        return (f'<rect x="{x+1.5:.1f}" y="{cy-3:.1f}" width="{cw-3:.1f}" height="6" '
                f'fill="none" stroke="{color}" stroke-width="1.2"/>')
    if c == "●":
        return f'<circle cx="{cx:.1f}" cy="{cy:.1f}" r="3.5" fill="{color}"/>'
    if c == "⛶":
        l, r, t, b, k = x + 1.5, x + cw - 1.5, cy - 4, cy + 4, 2.5
        path = (f"M{l},{t+k}V{t}H{l+k} M{r-k},{t}H{r}V{t+k} "
                f"M{r},{b-k}V{b}H{r-k} M{l+k},{b}H{l}V{b-k}")
        return f'<path d="{path}" fill="none" stroke="{color}" stroke-width="1.2"/>'
    return ""


class Screen:
    def __init__(self, cols, rows):
        self.cols, self.rows = cols, rows
        blank = (" ", None, None, False)
        self.cells = [[blank] * cols for _ in range(rows)]
        self.x = self.y = 0
        self.fg = self.bg = None
        self.bold = False
        self.cursor_visible = True
        self.buf = ""

    def clear(self):
        blank = (" ", None, None, False)
        self.cells = [[blank] * self.cols for _ in range(self.rows)]

    def sgr(self, params):
        ps = [int(p) if p else 0 for p in params.split(";")] if params else [0]
        i = 0
        while i < len(ps):
            p = ps[i]
            if p == 0:
                self.fg = self.bg = None; self.bold = False
            elif p == 1:
                self.bold = True
            elif p == 22:
                self.bold = False
            elif p in (38, 48) and i + 4 < len(ps) + 0 and ps[i + 1] == 2:
                rgb = "#%02x%02x%02x" % tuple(ps[i + 2:i + 5])
                if p == 38: self.fg = rgb
                else: self.bg = rgb
                i += 4
            elif p == 39:
                self.fg = None
            elif p == 49:
                self.bg = None
            i += 1

    def feed(self, data):
        self.buf += data
        out = self.buf
        i = 0
        n = len(out)
        while i < n:
            c = out[i]
            if c == "\x1b":
                if i + 1 >= n:
                    break
                if out[i + 1] == "[":
                    m = re.compile(r"\[([?0-9;]*)([ -/]*)([@-~])").match(out, i + 1)
                    if not m:
                        break  # incomplete sequence; wait for more
                    params, _, final = m.groups()
                    self.csi(params, final)
                    i = m.end()
                    continue
                if out[i + 1] in "()":
                    i += 3; continue
                i += 2; continue
            if c == "\r":
                self.x = 0
            elif c == "\n":
                self.y = min(self.y + 1, self.rows - 1)
            elif c >= " ":
                if self.x < self.cols and self.y < self.rows:
                    self.cells[self.y][self.x] = (c, self.fg, self.bg, self.bold)
                self.x += 1
            i += 1
        self.buf = out[i:]

    def csi(self, params, final):
        priv = params.startswith("?")
        p = params.lstrip("?")
        if final == "m" and not priv:
            self.sgr(p)
        elif final in "Hf":
            parts = (p.split(";") + ["", ""])[:2]
            self.y = (int(parts[0]) if parts[0] else 1) - 1
            self.x = (int(parts[1]) if parts[1] else 1) - 1
        elif final == "J" and p in ("2", "3"):
            self.clear()
        elif final in "hl" and priv and p == "25":
            self.cursor_visible = final == "h"

    def svg(self, dark_bg="#282c34", dark_fg="#abb2bf"):
        cw, ch = 9.0, 18.0
        w, h = self.cols * cw, self.rows * ch
        out = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{w:.0f}" height="{h:.0f}" '
               f'viewBox="0 0 {w:.0f} {h:.0f}">',
               f'<rect width="100%" height="100%" fill="{dark_bg}"/>',
               '<g font-family="Menlo, Consolas, \'DejaVu Sans Mono\', monospace" '
               'font-size="15" xml:space="preserve">']
        segments = []
        for y, row in enumerate(self.cells):
            x = 0
            while x < self.cols:
                ch0, fg, bg, bold = row[x]
                x2 = x
                text = ""
                while x2 < self.cols and row[x2][1:] == (fg, bg, bold):
                    text += row[x2][0]; x2 += 1
                if bg:
                    out.append(f'<rect x="{x*cw:.1f}" y="{y*ch:.1f}" width="{(x2-x)*cw:.1f}" '
                               f'height="{ch:.1f}" fill="{bg}"/>')
                color = fg or dark_fg
                for i, c in enumerate(text):
                    if c in BOX:
                        segments.extend(box_segments(c, (x + i) * cw, y * ch, cw, ch, color))
                    elif c in SHAPES:
                        out.append(glyph_svg(c, (x + i) * cw, y * ch, cw, ch, color))
                text = "".join(" " if c in BOX or c in SHAPES else c for c in text)
                if text.strip():
                    weight = ' font-weight="bold"' if bold else ""
                    out.append(f'<text x="{x*cw:.1f}" y="{y*ch+13.5:.1f}" fill="{fg or dark_fg}"'
                               f'{weight} textLength="{(x2-x)*cw:.1f}" '
                               f'lengthAdjust="spacingAndGlyphs">{html.escape(text)}</text>')
                x = x2
        out.extend(merged_lines(segments))
        if self.cursor_visible and 0 <= self.y < self.rows and 0 <= self.x < self.cols:
            ch0, fg, bg, bold = self.cells[self.y][self.x]
            out.append(f'<rect x="{self.x*cw:.1f}" y="{self.y*ch:.1f}" width="{cw:.1f}" '
                       f'height="{ch:.1f}" fill="{fg or dark_fg}"/>')
            if ch0.strip():
                out.append(f'<text x="{self.x*cw:.1f}" y="{self.y*ch+13.5:.1f}" '
                           f'fill="{bg or dark_bg}">{html.escape(ch0)}</text>')
        out.append("</g></svg>")
        return "\n".join(out)

    def text(self):
        return "\n".join("".join(c[0] for c in row).rstrip() for row in self.cells)


DECODER = codecs.getincrementaldecoder("utf-8")("replace")


def pump(fd, screen, seconds):
    """Read output until `seconds` pass with nothing new."""
    deadline = time.time() + seconds
    while True:
        r, _, _ = select.select([fd], [], [], max(0.0, deadline - time.time()))
        if not r:
            return
        try:
            data = os.read(fd, 65536)
        except OSError:
            return
        if not data:
            return
        screen.feed(DECODER.decode(data))
        deadline = time.time() + seconds


def main():
    spec = json.load(open(sys.argv[1]))
    repo = os.getcwd()
    cols, rows = spec.get("cols", 120), spec.get("rows", 36)
    workdir = tempfile.mkdtemp(prefix="tuishot-")
    for path in spec.get("copy", []):
        shutil.copytree(os.path.join(repo, path), os.path.join(workdir, path))
    cmd = [os.path.join(repo, spec["cmd"][0])] + spec["cmd"][1:]
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(workdir)
        env = dict(os.environ, TERM="xterm-256color", COLORTERM="truecolor")
        os.execvpe(cmd[0], cmd, env)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
    screen = Screen(cols, rows)
    pump(fd, screen, 1.5)
    for step in spec["steps"]:
        if "keys" in step:
            for ch in step["keys"]:
                os.write(fd, ch.encode())
                time.sleep(step.get("delay", 0.01))
            pump(fd, screen, step.get("settle", 0.8))
        elif "click" in step:
            marker, target = step["click"]
            rows_text = screen.text().split("\n")
            y = next(i for i, r in enumerate(rows_text) if marker in r)
            x = rows_text[y].index(target, rows_text[y].index(marker))
            for final in "Mm":
                os.write(fd, f"\x1b[<0;{x + 1};{y + 1}{final}".encode())
            pump(fd, screen, step.get("settle", 0.8))
        elif "wait" in step:
            pump(fd, screen, step["wait"])
        elif "snap" in step:
            path = os.path.join(repo, step["snap"])
            with open(path, "w") as f:
                f.write(screen.svg())
            if "--text" in sys.argv:
                with open(path[: -len(".svg")] + ".txt", "w") as f:
                    f.write(screen.text() + "\n")
    os.write(fd, b"\x1b:quit\r")
    pump(fd, screen, 0.5)
    shutil.rmtree(workdir, ignore_errors=True)


if __name__ == "__main__":
    main()
