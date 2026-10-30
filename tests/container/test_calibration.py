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
    ("flow_rate", "yolo", None, None, 1),
    ("flow_rate", "yolo_perfectionist", None, None, 1),
    ("flow_rate", "pass1", None, None, 1),
    ("flow_rate", "pass2", None, None, 1),
    ("pressure_advance", .01, .05, .02, .02),
    ("retraction", .2, .6, .2, .4),
    ("volumetric_flow", 5, 9, 2, 7),
]
previous = {}


def check_flow_tiles(text, names, pattern, baseline):
    # Compare the longest first-layer outside wall, the same native tile edge in each object.
    # Compare E per millimeter across those edges. Preview WIDTH already includes flow compensation.
    x = y = extrusion = 0.0
    relative = False
    tile = None
    outside = False
    longest = {}
    height = None
    first_height = {}
    for line in text.splitlines():
        object_match = re.match(r"; printing object flowrate_(m?\d+(?:\.\d+)?)_", line)
        if object_match:
            tile = object_match[1]
        if line.startswith("; stop printing object flowrate_"):
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
    assert set(longest) == names, longest
    baseline_length, baseline_amount = longest["0"]
    for tile, (length, amount) in longest.items():
        ratio = amount / length / (baseline_amount / baseline_length)
        # Writer precision: E five decimals and XY three decimals, propagated through the ratio.
        uncertainty = abs(ratio) * (.000005 / amount + .000005 / baseline_amount
            + math.sqrt(2) * .001 * (1 / length + 1 / baseline_length))
        modifier = float(tile.replace("m","-"))
        expected = (baseline + modifier) / baseline if pattern.startswith("yolo") else 1 + modifier / 100
        assert abs(ratio - expected) <= uncertainty, (tile, ratio, expected, uncertainty)


for step, low, high, increment, baseline in steps:
    previous.pop(step, None)
    artifact_name = step + ("_" + low if step == "flow_rate" else "")
    if len(sys.argv) > 4 and sys.argv[4] != artifact_name:
        previous[step] = baseline
        continue
    calibration = dict(step=step,baseline=baseline,previous_results=previous)
    if step == "flow_rate":
        calibration["flow_pattern"] = low
    else:
        calibration.update(lowest=low,highest=high,increment=increment)
    fields = {"schemaHash": schema["schema_hash"], "requestId": str(uuid.uuid4()),
              "calibration": json.dumps(calibration)}
    fields.update({key: json.dumps(dict(type=kind, name=name, inherits=name, **{"from": "system"}))
                   for key, (kind, name) in profiles.items()})
    if step == "volumetric_flow" and mode == "klipper":
        # Exercise the ramp with a first-layer speed below the calibration speeds.
        fields["presetProfile"] = json.dumps(dict(
            type="process", name="Calibration slowdown", setting_id="calibration-slowdown", inherits=profiles["presetProfile"][1],
            slow_down_layers="3", initial_layer_speed="10", **{"from": "user"}))
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
    (OUTPUT / (artifact_name + (".3mf" if mode == "bambu" else ".gcode"))).write_bytes(artifact)
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
    settings = dict(re.findall(r"^; (\w+) = (.*)$",text,re.M))
    expected_settings = {
        "temperature": {"brim_width":"5"},
        "pressure_advance": {"wall_loops":"2","bottom_shell_layers":"0","top_shell_layers":"0","brim_width":"6"},
        "retraction": {"wall_loops":"2","bottom_shell_layers":"3","top_shell_layers":"0","layer_height":"0.2"},
        "volumetric_flow": {"wall_loops":"1","bottom_shell_layers":"0","top_shell_layers":"0","layer_height":"0.32","spiral_mode":"1","outer_wall_line_width":"0.7","brim_width":"5"},
    }.get(step,{})
    for key,value in expected_settings.items():
        assert settings.get(key)==value,(step,key,settings.get(key),value)
    if step == "temperature":
        emitted = [int(v) for v in re.findall(r"^;LAYERCOVE_CALIBRATION Temperature (\d+)", text, re.M)]
        assert set(emitted) == {200, 205, 210} and emitted[0] == 210 and emitted[-1] == 200
    if step == "flow_rate":
        names = set(re.findall(r"flowrate_(m?\d+(?:\.\d+)?)", text))
        expected = {
            "pass1": {"0","5","10","15","20","m5","m10","m15","m20"},
            "pass2": {"0", *("m" + str(i) for i in range(1,10))},
            "yolo": {"0", *(f"{sign}0.0{i}" for sign in ("","m") for i in range(1,6))},
            "yolo_perfectionist": {"0","0.005","0.01","0.015","0.02","0.025","0.03","0.035","m0.005","m0.01","m0.015","m0.02","m0.025","m0.03","m0.035","m0.04"},
        }[low]
        assert names == expected, names
        check_flow_tiles(text, names, low, baseline)
    if step == "pressure_advance":
        values = [(float(v),float(z)) for v,z in re.findall(r"^;LAYERCOVE_CALIBRATION PressureAdvance ([\d.]+) Z=([\d.]+)", text, re.M)]
        assert values
        # The serialized Z may round an internal value just below an integer up to it.
        assert all(any(math.isclose(v,.01+band*.02) for band in
                       {math.floor(z),math.floor(math.nextafter(z,-math.inf))})
                   for v,z in values), values
        prefix = "SET_PRESSURE_ADVANCE ADVANCE=" if mode == "klipper" else "M900 K"
        assert all(f"{prefix}{value}" in text for value in [.01, .03, .05])
    if step == "retraction":
        start = re.search(r"^;LAYERCOVE_CALIBRATION Retraction ", text, re.M)
        end = re.search(r"^;LAYERCOVE_CALIBRATION_END$", text, re.M)
        assert start and end and start.start() < end.start()
        model = text[start.start():end.start()]
        amounts = {abs(float(v)) for v in re.findall(r"^G1 E(-?[\d.]+)", model, re.M)}
        assert {.2, .4, .6} <= amounts <= {.2, .4, .6, .8}, amounts
    if step == "volumetric_flow":
        values = [(float(v),float(z)) for v,z in re.findall(r"^;LAYERCOVE_CALIBRATION VolumetricFlow ([\d.]+) Z=([\d.]+)",text,re.M)]
        assert values
        for value,z in values:
            assert math.isclose(value,5+z*2,abs_tol=1e-5),(value,z)
        area = .32 * (.7 - .32 * (1 - math.pi / 4))
        initial = float(re.search(r"^; initial_layer_speed = ([\d.]+)$", text, re.M)[1])
        slowdown = int(re.search(r"^; slow_down_layers = (\d+)$", text, re.M)[1])
        layer = -1
        wall = False
        expected = None
        checked = set()
        feedrate = None
        for line in text.splitlines():
            marker = re.match(r";LAYERCOVE_CALIBRATION VolumetricFlow ([\d.]+)", line)
            if marker:
                layer += 1
                target = math.floor(float(marker[1]) / area + .5)
                expected = initial if layer == 0 else target
                if 0 < layer < slowdown:
                    expected = min(target, initial + (target - initial) * layer / slowdown)
            if line == ";LAYERCOVE_CALIBRATION_END":
                expected = None
            if line.startswith((";TYPE:", "; FEATURE:")):
                wall = "Outer wall" in line
            code = line.split(";", 1)[0]
            fields = {k: float(v) for k, v in re.findall(r"\b([XYEF])(-?(?:\d+(?:\.\d*)?|\.\d+))", code)}
            feedrate = fields.get("F", feedrate)
            if expected is not None and wall and code.startswith("G1 ") and fields.get("E", 0) > 0 and ("X" in fields or "Y" in fields):
                # F has three decimals. Pinned PlaceholderParser.cpp uses the
                # stream default of six significant digits for speed expressions.
                uncertainty = .5 * 10**-3 + 60 * .5 * 10**(math.floor(math.log10(expected)) - 5)
                assert math.isclose(feedrate, 60 * expected, abs_tol=uncertainty), (layer, feedrate, expected)
                checked.add(layer)
        assert checked == set(range(len(values))), checked
    expected_names = {"temperature":"temperature_tower","pressure_advance":"tower_with_seam","retraction":"retraction_tower","volumetric_flow":"SpeedTestStructure"}
    if step in expected_names:
        assert expected_names[step] in text, expected_names[step]
        heights = [float(z) for z in re.findall(r"^;LAYERCOVE_CALIBRATION \w+ [\d.]+ Z=([\d.]+)",text,re.M)]
        expected_height = {"temperature":30,"pressure_advance":3,"retraction":3.4,"volumetric_flow":2.5}[step]
        layer = .32 if step=="volumetric_flow" else .2
        assert abs(max(heights)-expected_height)<=layer/2+1e-5,(max(heights),expected_height)
    layers = [float(z) for z in re.findall(r"^;(?: Z_HEIGHT:|Z:)\s*([\d.]+)", text, re.M)]
    samples = sorted({float(v) for v in re.findall(r"^;LAYERCOVE_CALIBRATION \w+ ([\d.]+)",text,re.M)})
    print(json.dumps({"provider": mode, "step": step, "flow_pattern": low if step=="flow_rate" else None,
                      "bytes": len(artifact), "first_z": min(layers), "top_z": max(layers),
                      "tested_values": samples, "objects": sorted(set(re.findall(r"^; printing object (\S+)",text,re.M)))}), flush=True)
    previous[step] = baseline
