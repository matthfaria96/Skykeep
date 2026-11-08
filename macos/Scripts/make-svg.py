#!/usr/bin/env python3
"""Skykeep's icon as SVG, with the same geometry as make-icon.swift (the 1024 grid, y up,
flipped once at the top). Usage: make-svg.py <flat|tray|notify> <out.svg>"""
import math, sys

variant, out = sys.argv[1], sys.argv[2]

def squircle(x, y, w, h, n=4.6, steps=360):
    cx, cy, a, b = x + w / 2, y + h / 2, w / 2, h / 2
    pts = []
    for i in range(steps):
        t = i / steps * 2 * math.pi
        c, s = math.cos(t), math.sin(t)
        pts.append((cx + a * math.copysign(abs(c) ** (2 / n), c), cy + b * math.copysign(abs(s) ** (2 / n), s)))
    return "M" + " L".join(f"{px:.1f} {py:.1f}" for px, py in pts) + " Z"

def circle(cx, cy, r):
    return f"M{cx - r} {cy} a{r} {r} 0 1 0 {2 * r} 0 a{r} {r} 0 1 0 {-2 * r} 0 Z"

def rrect(x, y, w, h, r):
    return (f"M{x + r} {y} H{x + w - r} A{r} {r} 0 0 1 {x + w} {y + r} V{y + h - r} A{r} {r} 0 0 1 {x + w - r} {y + h} "
            f"H{x + r} A{r} {r} 0 0 1 {x} {y + h - r} V{y + r} A{r} {r} 0 0 1 {x + r} {y} Z")

# Each part filled on its own: as one path, overlapping parts drawn the other way round
# cancel out under SVG's fill rule.
CLOUD_PARTS = [rrect(214, 250, 596, 220, 110), circle(380, 455, 125), circle(528, 520, 168), circle(688, 455, 112)]

def shapes(attrs):
    return "".join(f'<path d="{d}" {attrs}/>' for d in CLOUD_PARTS)
KEY_HEAD = circle(512, 444, 46)
KEY_SLOT = "M490 430 L534 430 L546 302 Q547 292 536 292 L488 292 Q477 292 478 302 Z"

defs = """
<linearGradient id="sky" gradientUnits="userSpaceOnUse" x1="512" y1="924" x2="512" y2="100">
  <stop offset="0" stop-color="#6CC6FF"/><stop offset="0.55" stop-color="#2F86F6"/><stop offset="1" stop-color="#1D4FD8"/></linearGradient>
<radialGradient id="glow" gradientUnits="userSpaceOnUse" cx="690" cy="700" r="420">
  <stop offset="0" stop-color="#FFF2C4" stop-opacity="0.55"/><stop offset="1" stop-color="#FFF2C4" stop-opacity="0"/></radialGradient>
<radialGradient id="halo" gradientUnits="userSpaceOnUse" cx="668" cy="612" r="250">
  <stop offset="0.48" stop-color="#FFD866" stop-opacity="0.65"/><stop offset="1" stop-color="#FFD866" stop-opacity="0"/></radialGradient>
<linearGradient id="sun" gradientUnits="userSpaceOnUse" x1="668" y1="744" x2="668" y2="480">
  <stop offset="0" stop-color="#FFE27A"/><stop offset="1" stop-color="#FFB531"/></linearGradient>
<linearGradient id="cloud" gradientUnits="userSpaceOnUse" x1="512" y1="682" x2="512" y2="250">
  <stop offset="0" stop-color="#FFFFFF"/><stop offset="0.5" stop-color="#F3F7FF"/><stop offset="1" stop-color="#D5E3FB"/></linearGradient>
<linearGradient id="key" gradientUnits="userSpaceOnUse" x1="512" y1="490" x2="512" y2="292">
  <stop offset="0" stop-color="#1F55D6"/><stop offset="1" stop-color="#3D86F5"/></linearGradient>
<linearGradient id="sheen" gradientUnits="userSpaceOnUse" x1="512" y1="924" x2="512" y2="640">
  <stop offset="0" stop-color="#FFFFFF" stop-opacity="0.28"/><stop offset="1" stop-color="#FFFFFF" stop-opacity="0"/></linearGradient>
<filter id="soft" x="-20%" y="-20%" width="140%" height="140%"><feGaussianBlur stdDeviation="23"/></filter>
"""

def cloud(shadow=True):
    parts = []
    if shadow:
        parts.append(f'<g transform="translate(0 -22)" fill="#0A2A80" fill-opacity="0.4" filter="url(#soft)">{shapes("")}</g>')
    parts.append(shapes('fill="url(#cloud)"'))
    parts.append(f'<path d="{KEY_HEAD}" fill="url(#key)"/><path d="{KEY_SLOT}" fill="url(#key)"/>')
    return "\n".join(parts)

if variant == "flat":
    body = (24, 24, 976, 976)
    k = 976 / 824
    tile = squircle(*body)
    art = f"""<clipPath id="tile"><path d="{tile}"/></clipPath>
<g clip-path="url(#tile)"><g transform="translate(512 512) scale({k:.4f}) translate(-512 -512)">
<rect x="-200" y="-200" width="1424" height="1424" fill="url(#sky)"/>
<rect x="-200" y="-200" width="1424" height="1424" fill="url(#glow)"/>
<path d="{circle(668, 612, 250)}" fill="url(#halo)"/>
<path d="{circle(668, 612, 132)}" fill="url(#sun)"/>
{cloud()}
<rect x="-200" y="-200" width="1424" height="1424" fill="url(#sheen)"/>
</g></g>"""
elif variant == "tray":
    art = f"""<g transform="translate(512 512) scale(1.22) translate(-512 -466)">
{shapes('fill="none" stroke="#151A22" stroke-width="46"')}
{cloud(shadow=False)}</g>"""
elif variant == "notify":
    art = f"""<path d="{circle(512, 512, 512)}" fill="url(#sky)"/>
<g transform="translate(512 502) scale(1.18) translate(-512 -466)">{cloud()}</g>"""
else:
    raise SystemExit("unknown variant")

svg = f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024"><defs>{defs}</defs>
<g transform="matrix(1 0 0 -1 0 1024)">{art}</g></svg>
"""
open(out, "w").write(svg)
