#!/usr/bin/env python3
"""Inspect all calibration artifacts from the real prebuilt engine, without printer I/O."""
import json
import sys
import math
import re
import io
import hashlib
import zipfile
import uuid
import urllib.request
from pathlib import Path

BASE = sys.argv[1]
OUTPUT = Path(sys.argv[2])
OUTPUT.mkdir(parents=True, exist_ok=True)
mode = sys.argv[3] if len(sys.argv) > 3 else "bambu"
schema = json.load(urllib.request.urlopen(BASE + "/schema/process"))
profiles = {
    "printerProfile": ("machine", "Bambu Lab X1 Carbon 0.4 nozzle"),
    "presetProfile": ("process", "0.20mm Standard @BBL X1C"),
    "filamentProfile": ("filament", "Bambu PLA Basic @BBL X1C"),
}
if mode == "klipper":
    profiles = {
        "printerProfile": ("machine", "Voron 2.4 250 0.4 nozzle"),
        "presetProfile": ("process", "0.20mm Standard @Voron"),
        "filamentProfile": ("filament", "Generic PLA @System"),
    }
steps = [
    ("temperature", 200, 210, 5, 210),
    ("flow_rate", .9, 1.1, .1, 1),
    ("pressure_advance", .01, .05, .02, .02),
    ("retraction", .2, .6, .2, .4),
    ("volumetric_flow", 5, 9, 2, 7),
]
previous = {}


def check_flow_tiles(text):
    # Compare the longest first-layer outside wall, the same cuboid edge in each tile.
    # Compare E per millimeter across those edges. Preview WIDTH already includes flow compensation.
    x = y = extrusion = 0.0
    relative = False
    tile = None
    outside = False
    longest = {}
    height = None
    first_height = {}
    for line in text.splitlines():
        object_match = re.match(r"; printing object sample_(\d+)_", line)
        if object_match:
            tile = int(object_match[1])
        if line.startswith("; stop printing object"):
            tile = None
        if line.startswith((";TYPE:", "; FEATURE:")):
            outside = "Outer wall" in line
        height_match = re.match(r";(?: Z_HEIGHT:|Z:)\s*([\d.]+)", line)
        if height_match:
            height = float(height_match[1])
        command = line.split(";", 1)[0].strip()
        if command == "M83":
            relative = True
        elif command == "M82":
            relative = False
        fields = {key: float(value) for key, value in re.findall(r"\b([XYE])(-?(?:\d+(?:\.\d*)?|\.\d+))", command)}
        if command.startswith("G92 "):
            extrusion = fields.get("E", extrusion)
        if not re.match(r"G[0123] ", command):
            continue
        next_x, next_y = fields.get("X", x), fields.get("Y", y)
        length = math.hypot(next_x - x, next_y - y)
        amount = fields.get("E", 0) if relative else fields.get("E", extrusion) - extrusion
        if command.startswith("G1 ") and tile and outside and length and amount > 0:
            first_height.setdefault(tile, height)
            if height == first_height[tile] and length > longest.get(tile, (0, 0))[0]:
                longest[tile] = (length, amount)
        x, y = next_x, next_y
        if "E" in fields:
            extrusion = fields["E"] if not relative else extrusion + fields["E"]
    assert set(longest) == {1, 2, 3}, longest
    baseline_length, baseline_amount = longest[2]
    for tile, (length, amount) in longest.items():
        ratio = amount / length / (baseline_amount / baseline_length)
        # Writer precision: E five decimals and XY three decimals, propagated through the ratio.
        uncertainty = abs(ratio) * (.000005 / amount + .000005 / baseline_amount
            + math.sqrt(2) * .001 * (1 / length + 1 / baseline_length))
        assert abs(ratio - (.9 + (tile - 1) * .1)) <= uncertainty, (tile, ratio, uncertainty)


for step, low, high, increment, baseline in steps:
    fields = {"schemaHash": schema["schema_hash"], "requestId": str(uuid.uuid4()),
              "calibration": json.dumps(dict(step=step, lowest=low, highest=high,
                  increment=increment, baseline=baseline, previous_results=previous))}
    fields.update({key: json.dumps(dict(type=kind, name=name, inherits=name, **{"from": "system"}))
                   for key, (kind, name) in profiles.items()})
    if mode == "bambu":
        fields["exportType"] = "3mf"
    boundary = "layercove-calibration-test"
    body = b""
    for key, value in fields.items():
        body += (f'--{boundary}\r\nContent-Disposition: form-data; name="{key}"\r\n\r\n{value}\r\n').encode()
    body += f"--{boundary}--\r\n".encode()
    request = urllib.request.Request(BASE + "/slice", data=body,
        headers={"Content-Type": "multipart/form-data; boundary=" + boundary})
    try:
        response = urllib.request.urlopen(request)
    except urllib.error.HTTPError as error:
        raise RuntimeError(error.read().decode()) from error
    artifact = response.read()
    (OUTPUT / (step + (".3mf" if mode == "bambu" else ".gcode"))).write_bytes(artifact)
    assert float(response.headers["x-print-time-seconds"]) > 0
    if mode == "bambu":
        with zipfile.ZipFile(io.BytesIO(artifact)) as archive:
            assert archive.testzip() is None
            names = [name for name in archive.namelist() if name.endswith(".gcode")]
            assert len(names) == 1
            artifact = archive.read(names[0])
            checksum = archive.read(names[0] + ".md5").decode().strip()
            assert checksum.lower() == hashlib.md5(artifact).hexdigest(), checksum
    assert b"G1 " in artifact and b"fixture-gcode" not in artifact
    text = artifact.decode()
    if step == "temperature":
        emitted = [int(v) for v in re.findall(r"^;LAYERCOVE_CALIBRATION Temperature (\d+)", text, re.M)]
        assert set(emitted) == {200, 205, 210} and emitted[0] == 210 and emitted[-1] == 200
    if step == "flow_rate":
        check_flow_tiles(text)
    if step == "pressure_advance":
        assert {float(v) for v in re.findall(r"^;LAYERCOVE_CALIBRATION PressureAdvance ([\d.]+)", text, re.M)} == {.01, .03, .05}
        prefix = "SET_PRESSURE_ADVANCE ADVANCE=" if mode == "klipper" else "M900 K"
        assert all(f"{prefix}{value}" in text for value in [.01, .03, .05])
    if step == "retraction":
        start = re.search(r"^;LAYERCOVE_CALIBRATION Retraction ", text, re.M)
        end = re.search(r"^;LAYERCOVE_CALIBRATION_END$", text, re.M)
        assert start and end and start.start() < end.start()
        model = text[start.start():end.start()]
        amounts = {abs(float(v)) for v in re.findall(r"^G1 E(-?[\d.]+)", model, re.M)}
        assert amounts == {.2, .4, .6}, amounts
    if step == "volumetric_flow":
        assert {float(v) for v in re.findall(r"^;LAYERCOVE_CALIBRATION VolumetricFlow ([\d.]+)", text, re.M)} == {5, 7, 9}
        area = .2 * (.7 - .2 * (1 - math.pi / 4))
        feedrates = {float(v) for v in re.findall(r"\bF([\d.]+)", text)}
        # Orca emits feedrate with three decimal places. Each requested wall speed must survive slicing.
        for value in [5, 7, 9]:
            expected = 60 * value / area
            assert any(abs(feed - expected) <= .0005 for feed in feedrates), (value, expected)
    print(json.dumps({"provider": mode, "step": step, "bytes": len(artifact)}), flush=True)
    previous[step] = baseline
