#!/usr/bin/env python3
"""Generate assets/icon.svg for the 168-Hour Week Clock.

Run from anywhere: `python3 scripts/gen_icon.py` (re)writes ../assets/icon.svg.
Raster assets (PNG/ICO/ICNS) are produced from this SVG by build-icons.sh.
"""
import math
import os

W = 512.0
C = W / 2.0  # center

# Palette (matches the app)
DAY = ["#E63845", "#F2732B", "#F7C74F", "#6BBA66", "#33A899", "#4580C7", "#9E59D9"]
GRAY = "#808A99"      # minor hour-tick gray
GREEN = "#45D96B"     # work-hour green
FACE = "#14171C"      # dark clock face
HOUR = "#EDF2FC"      # hour hand
MIN = "#BDCCEB"       # minute hand
HUB = "#EDF2FC"
DOT = "#F55245"       # red hub center

# Ring geometry (radii are centerlines; t = stroke thickness)
FACE_R = 248.0
COLOR_R, COLOR_T = 221.0, 54.0     # spans 194..248
INNER_R, INNER_T = 170.0, 16.0     # spans 162..178


def pt(theta_deg, r):
    """Point on radius r at angle measured clockwise from the top (12 o'clock)."""
    t = math.radians(theta_deg)
    return (C + r * math.sin(t), C - r * math.cos(t))


def arc(a1, a2, r):
    x1, y1 = pt(a1, r)
    x2, y2 = pt(a2, r)
    large = 1 if (a2 - a1) > 180 else 0
    return f"M {x1:.3f} {y1:.3f} A {r:.3f} {r:.3f} 0 {large} 1 {x2:.3f} {y2:.3f}"


out = []
out.append(
    f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W:.0f} {W:.0f}" '
    f'width="{W:.0f}" height="{W:.0f}">'
)
out.append('  <title>168-Hour Week Clock</title>')

# Dark round face (background; the color ring covers its outer band).
out.append(f'  <circle cx="{C}" cy="{C}" r="{FACE_R}" fill="{FACE}"/>')

# Outer ring: seven day-colored arcs, clockwise from the top (Sunday).
out.append('  <g fill="none" stroke-width="%.0f">' % COLOR_T)
for d in range(7):
    a1, a2 = d * 360.0 / 7.0, (d + 1) * 360.0 / 7.0
    out.append(f'    <path d="{arc(a1, a2, COLOR_R)}" stroke="{DAY[d]}"/>')
out.append('  </g>')

# Inner ring: full gray ring + solid green Mon-Fri 9am-5pm arcs.
out.append(f'  <circle cx="{C}" cy="{C}" r="{INNER_R}" fill="none" '
           f'stroke="{GRAY}" stroke-width="{INNER_T:.0f}"/>')
out.append('  <g fill="none" stroke-width="%.0f" stroke-linecap="butt">' % INNER_T)
for d in range(1, 6):  # Mon..Fri
    a1 = (d * 24 + 9) / 168.0 * 360.0
    a2 = (d * 24 + 17) / 168.0 * 360.0
    out.append(f'    <path d="{arc(a1, a2, INNER_R)}" stroke="{GREEN}"/>')
out.append('  </g>')

# Hands (rectangles). Hour points up (Sun 00:00); minute points right (:15).
hw, hlen, htail = 22.0, 100.0, 26.0
out.append(f'  <rect x="{C - hw/2:.1f}" y="{C - hlen:.1f}" width="{hw:.0f}" '
           f'height="{hlen + htail:.0f}" rx="{hw/2:.0f}" fill="{HOUR}"/>')
mw, mlen, mtail = 14.0, 150.0, 30.0
out.append(f'  <rect x="{C - mtail:.1f}" y="{C - mw/2:.1f}" width="{mlen + mtail:.0f}" '
           f'height="{mw:.0f}" rx="{mw/2:.0f}" fill="{MIN}"/>')

# Center hub.
out.append(f'  <circle cx="{C}" cy="{C}" r="16" fill="{HUB}"/>')
out.append(f'  <circle cx="{C}" cy="{C}" r="7" fill="{DOT}"/>')

out.append('</svg>')
out.append('')

dest = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "assets", "icon.svg")
dest = os.path.normpath(dest)
os.makedirs(os.path.dirname(dest), exist_ok=True)
with open(dest, "w", encoding="utf-8") as f:
    f.write("\n".join(out))
print(f"wrote {dest}")
