"""Bake the Android system splash from the application's shared pixel artwork.

Only Python's standard library is needed. Generated resources live in target/;
the source of the silhouette and colors remains mushroom.rs and theme.json.
"""
import argparse
import copy
from collections import defaultdict
import heapq
import json
from pathlib import Path
import re
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parent.parent
ANDROID = "http://schemas.android.com/apk/res/android"
AAPT = "http://schemas.android.com/aapt"
ET.register_namespace("android", ANDROID)
ET.register_namespace("aapt", AAPT)


def element(tag, parent=None, **attributes):
    node = ET.Element(tag, {"{" + ANDROID + "}" + k: str(v) for k, v in attributes.items()})
    if parent is not None:
        parent.append(node)
    return node


def mix(a, b, percent):
    return tuple((x * (100 - percent) + y * percent + 50) // 100 for x, y in zip(a, b))


def generate(output):
    theme = json.loads((ROOT / "assets/theme.json").read_text(encoding="utf-8"))
    rgb = lambda key: tuple(bytes.fromhex(theme[key][1:]))
    bg, primary, secondary, stem = map(rgb, ("background", "indigo", "teal", "logo_stem"))
    white = (255, 255, 255)
    # Same palette as mushroom::palette for the default Neon theme.
    palette = dict(zip("RHEDWVQSLABMNJ", (
        primary, mix(primary, white, 24), mix(primary, bg, 16), mix(primary, bg, 30),
        secondary, mix(secondary, white, 16), mix(secondary, primary, 15), stem,
        mix(stem, white, 60), mix(stem, bg, 21), mix(stem, bg, 12),
        mix(secondary, stem, 46), mix(secondary, bg, 35), mix(secondary, stem, 70),
    )))
    source = (ROOT / "native/src/mushroom.rs").read_text(encoding="utf-8")
    hero = source.split("pub const HERO:", 1)[1].split("= [", 1)[1].split("];", 1)[0]
    rows = re.findall(r'b"([.A-Z]+)"', hero)
    if len(rows) != 72 or any(len(row) != 80 for row in rows):
        raise ValueError("Shared HERO dimensions changed; review splash framing")
    if set("".join(rows)) - set(palette) - {"."}:
        raise ValueError("Unknown shared artwork palette entry")
    spots_source = source.split("pub const HERO_SPOTS:", 1)[1].split("=", 1)[1].split(";", 1)[0]
    spots = [tuple(map(int, match)) for match in re.findall(r"\((\d+),\s*(\d+),\s*(\d+),\s*(\d+)\)", spots_source)]
    if len(spots) != 3:
        raise ValueError("Shared spot animation regions changed")

    # Follow the actual connected branches, as MyceliumGrowth does in the UI.
    # Quantize distance into six short reveal waves for Android's vector animator.
    distance, pending = {}, []
    for x in range(80):
        if rows[48][x] != ".":
            delay = abs(x - 39) * 12
            distance[48, x] = delay
            heapq.heappush(pending, (delay, 48, x))
    while pending:
        d, y, x = heapq.heappop(pending)
        if d != distance[y, x]:
            continue
        for dy in (-1, 0, 1):
            for dx in (-1, 0, 1):
                ny, nx = y + dy, x + dx
                if not (48 <= ny < 72 and 0 <= nx < 80) or rows[ny][nx] == ".":
                    continue
                candidate = d + (141 if dx and dy else 100) + (ny * 37 + nx * 19) % 61
                if candidate < distance.get((ny, nx), 1 << 30):
                    distance[ny, nx] = candidate
                    heapq.heappush(pending, (candidate, ny, nx))
    longest = max(distance.values())
    paths = defaultdict(list)
    for y, row in enumerate(rows):
        for x, glyph in enumerate(row):
            if glyph == ".":
                continue
            group = "body"
            if y >= 48:
                if (y, x) not in distance:
                    continue
                group = "root" + str(min(5, distance[y, x] * 6 // (longest + 1)))
            elif glyph in "WVQ":
                group = "spot" + str(next(i for i, (left, top, w, h) in enumerate(spots)
                                          if left <= x < left + w and top <= y < top + h))
            paths[group, glyph].append(f"M{x},{y}h1v1h-1z")

    vector = element("vector", width="288dp", height="288dp", viewportWidth=108, viewportHeight=108)
    # All 80x72 pixels fit inside Android's central 72dp safe circle.
    frame = element("group", vector, scaleX="0.85", scaleY="0.85", translateX=20, translateY=23.4)
    motion = element("group", frame, name="mushroom", pivotX=40, pivotY=36)
    animated = element("animated-vector")
    drawable = ET.SubElement(animated, "{" + AAPT + "}attr", {"name": "android:drawable"})
    drawable.append(vector)

    def animate(name, prop, start, end, delay, duration, kind="floatType", parent=None):
        if parent is None:
            target = element("target", animated, name=name)
            parent = ET.SubElement(target, "{" + AAPT + "}attr", {"name": "android:animation"})
        return element("objectAnimator", parent, propertyName=prop, valueFrom=start, valueTo=end,
                       valueType=kind, startOffset=delay, duration=duration,
                       interpolator="@android:interpolator/fast_out_slow_in")

    color = lambda c: "#" + "".join(f"{v:02X}" for v in c)
    for (group, glyph), commands in sorted(paths.items()):
        name = group + "_" + glyph
        attributes = dict(name=name, fillColor=color(palette[glyph]), pathData="".join(commands))
        if group.startswith("root"):
            attributes["fillAlpha"] = "0"
        element("path", motion, **attributes)
        if group.startswith("root"):
            animate(name, "fillAlpha", 0, 1, 80 + int(group[4:]) * 95, 240)
        elif group.startswith("spot"):
            target = element("target", animated, name=name)
            attr = ET.SubElement(target, "{" + AAPT + "}attr", {"name": "android:animation"})
            base, glow = color(palette[glyph]), color(mix(palette[glyph], stem, 35))
            pulse = animate(name, "fillColor", base, glow, int(group[4:]) * 60, 600, "colorType", attr)
            pulse.set("{" + ANDROID + "}repeatCount", "-1")
            pulse.set("{" + ANDROID + "}repeatMode", "reverse")
    animate("mushroom", "translateY", 2, 0, 0, 420)
    output = Path(output) / "drawable"
    output.mkdir(parents=True, exist_ok=True)
    ET.indent(animated)
    ET.ElementTree(animated).write(output / "startup_mushroom.xml", encoding="utf-8", xml_declaration=True)
    # NativeActivity takes ownership of the application's surface. Its handoff
    # window continues the glow with complete roots, without replaying growth.
    holding = copy.deepcopy(animated)
    for target in list(holding):
        if target.tag == "target" and not target.get("{" + ANDROID + "}name", "").startswith("spot"):
            holding.remove(target)
    for path in holding.iter("path"):
        path.attrib.pop("{" + ANDROID + "}fillAlpha", None)
    ET.ElementTree(holding).write(output / "startup_mushroom_holding.xml", encoding="utf-8", xml_declaration=True)
    # Pre-Android-12 starting windows cannot animate; show the same complete art
    # until Slint's real, animated loading scene draws its first frame.
    for path in vector.iter("path"):
        path.attrib.pop("{" + ANDROID + "}fillAlpha", None)
    ET.indent(vector)
    ET.ElementTree(vector).write(output / "startup_mushroom_static.xml", encoding="utf-8", xml_declaration=True)
    print("Generated Android launch artwork from shared HERO and theme")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True)
    generate(parser.parse_args().output)
