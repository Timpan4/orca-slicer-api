#!/usr/bin/env python3
"""Restricted-container smoke test against a real OrcaSlicer slice."""
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

ENGINE = os.environ.get("CONTAINER_ENGINE", "docker")
IMAGE = sys.argv[1] if len(sys.argv) == 2 else (_ for _ in ()).throw(SystemExit("usage: test_runtime.py IMAGE"))
SUFFIX = f"{os.getpid()}"
RUNTIME = f"orca-api-runtime-{SUFFIX}"
VOLUME = f"orca-api-data-{SUFFIX}"
DIGEST = "sha256:" + "1" * 64
ROOT = os.path.join(tempfile.gettempdir(), f"orca-api-runtime-{SUFFIX}")
HEADERS, ARTIFACT = ROOT + ".headers", ROOT + ".gcode"
CUBE = os.path.join(os.path.dirname(__file__), "../../fixtures/container/cube.stl")
PROFILE_STUBS = {
    "printer.json": {
        "type": "machine",
        "name": "Bambu Lab X1 Carbon 0.4 nozzle",
        "inherits": "Bambu Lab X1 Carbon 0.4 nozzle",
        "from": "system",
    },
    "process.json": {
        "type": "process",
        "name": "0.20mm Standard @BBL X1C",
        "inherits": "0.20mm Standard @BBL X1C",
        "from": "system",
    },
    "filament.json": {
        "type": "filament",
        "name": "Bambu PLA Basic @BBL X1C",
        "inherits": "Bambu PLA Basic @BBL X1C",
        "from": "system",
    },
}


def run(*args, capture=False, check=True):
    return subprocess.run([ENGINE, *args], check=check, text=True,
                          stdout=subprocess.PIPE if capture else None,
                          stderr=subprocess.PIPE if capture else None)


def cleanup():
    subprocess.run(
        [ENGINE, "rm", "--force", RUNTIME],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    subprocess.run([ENGINE, "volume", "rm", "--force", VOLUME], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    for path in (HEADERS, ARTIFACT):
        try:
            os.unlink(path)
        except FileNotFoundError:
            pass


def get_json(url):
    try:
        with urllib.request.urlopen(url) as response:
            return json.load(response)
    except urllib.error.HTTPError:
        raise
    except urllib.error.URLError as error:
        raise ConnectionError(str(error)) from error


def canonical(value):
    if isinstance(value, dict):
        return {key: canonical(value[key]) for key in sorted(value)}
    if isinstance(value, list):
        return [canonical(item) for item in value]
    return value


def schema_hash(schema):
    process = {key: schema[key] for key in ("pages", "options", "scopes", "samples")}
    encoded = json.dumps(canonical(process), separators=(",", ":"), ensure_ascii=False).encode()
    return hashlib.sha256(encoded).hexdigest()


cleanup()
try:
    run("volume", "create", VOLUME, capture=True)
    with tempfile.TemporaryDirectory(prefix="orca-api-profiles-") as profiles:
        for name, stub in PROFILE_STUBS.items():
            with open(os.path.join(profiles, name), "w", encoding="utf-8") as stream:
                json.dump(stub, stream, separators=(",", ":"))
        run("run", "--detach", "--name", RUNTIME, "--read-only", "--user", "10001:10001",
            "--cap-drop", "ALL", "--security-opt", "no-new-privileges", "--volume", f"{VOLUME}:/app/data",
            "--publish", "127.0.0.1::3000", "--env", f"ORCA_IMAGE_DIGEST={DIGEST}", IMAGE, capture=True)
        inspection = json.loads(run("inspect", RUNTIME, capture=True).stdout)[0]
        assert inspection["Config"]["User"] == "10001:10001"
        assert inspection["HostConfig"]["ReadonlyRootfs"] is True
        assert inspection["HostConfig"]["Privileged"] is False
        assert inspection["HostConfig"].get("CapDrop")
        security_opts = {str(opt).lower() for opt in inspection["HostConfig"].get("SecurityOpt", [])}
        assert security_opts & {"no-new-privileges", "no-new-privileges:true", "no-new-privileges=true"}
        assert [mount["Destination"] for mount in inspection["Mounts"]] == ["/app/data"]
        assert inspection["HostConfig"].get("Tmpfs", {}) == {}
        assert not any(value.startswith("ORCA_BRIDGE_PATH=") for value in inspection["Config"]["Env"])
        assert "ORCA_PROFILE_SOURCE_PATH=/app/orca/resources/profiles" in inspection["Config"]["Env"]
        endpoint = run("port", RUNTIME, "3000/tcp", capture=True).stdout.strip()
        base = f"http://{endpoint}"
        while True:
            try:
                health = get_json(f"{base}/health")
                break
            except ConnectionError:
                state = json.loads(run("inspect", RUNTIME, capture=True).stdout)[0]["State"]
                if not state["Running"]:
                    logs = run("logs", RUNTIME, capture=True, check=False)
                    raise RuntimeError(f"container exited before readiness: {logs.stderr or logs.stdout}")
        assert health["status"] == "ok" and health["contract_version"] == "1"
        assert health["engine"] == {"name": "OrcaSlicer", "version": "2.4.2", "commit": "8500fcdccaa10b5099ac20d252af3a7c560046f1"}
        assert health["image_identity"]["digest"] == DIGEST
        proc_status = {
            key: value.strip()
            for line in run("exec", RUNTIME, "cat", "/proc/1/status", capture=True).stdout.splitlines()
            if ":" in line
            for key, value in [line.split(":", 1)]
        }
        assert int(proc_status["CapEff"], 16) == 0
        assert int(proc_status["CapBnd"], 16) == 0
        assert proc_status["NoNewPrivs"] == "1"
        capabilities = get_json(f"{base}/capabilities")
        assert capabilities["capabilities"] == {"process_schema": True, "model_state": False, "progress": True, "cancel": False}
        schema = get_json(f"{base}/schema/process")
        assert schema["schema_hash"] == capabilities["schema_hash"] == health["schema_hash"] == schema_hash(schema)
        assert len(schema["pages"]) == 6
        assert sum(len(page["groups"]) for page in schema["pages"]) == 41
        assert sum(len(group["options"]) for page in schema["pages"] for group in page["groups"]) == 342
        assert len(schema["options"]) == len(schema["scopes"]) == len(schema["samples"]) == 342
        profiles_json = get_json(f"{base}/profiles/bundled")
        assert profiles_json.get("printer") and profiles_json.get("process") and profiles_json.get("filament")
        for kind in ("printer", "filament"):
            profile_schema = get_json(f"{base}/schema/{kind}")
            assert profile_schema["schema_hash"] == schema_hash(profile_schema)
            assert profile_schema["engine"] == health["engine"]
            assert profile_schema["image_identity"] == health["image_identity"]
            assert profile_schema["options"]
        with tempfile.TemporaryDirectory(prefix="orca-calibration-artifacts-") as artifacts:
            for provider in ("bambu", "klipper"):
                subprocess.run([sys.executable, str(Path(__file__).with_name("test_calibration.py")),
                                base, str(Path(artifacts) / provider), provider], check=True)
        request_id = f"real-stl-{SUFFIX}"
        command = ["curl", "--silent", "--show-error", "--fail-with-body", "--dump-header", HEADERS, "--output", ARTIFACT,
                   "--form", f"file=@{CUBE};type=application/sla", "--form", f"printerProfile=@{profiles}/printer.json;type=application/json",
                   "--form", f"presetProfile=@{profiles}/process.json;type=application/json", "--form", f"filamentProfile=@{profiles}/filament.json;type=application/json",
                   "--form", f"schemaHash={schema['schema_hash']}", "--form", f"requestId={request_id}", f"{base}/slice"]
        slicer = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        observed = None
        while slicer.poll() is None:
            try:
                progress = get_json(f"{base}/slice/progress/{request_id}")
                if progress.get("stage") == "running" and 0 < progress.get("total_percent", 0) < 100:
                    observed = progress
            except urllib.error.HTTPError as error:
                if error.code != 404:
                    raise RuntimeError(f"HTTP {error.code} polling progress") from error
            except ConnectionError:
                pass
            time.sleep(0.01)
        stdout, stderr = slicer.communicate()
        if slicer.returncode:
            raise RuntimeError(f"slice curl failed ({slicer.returncode}): {stderr or stdout}")
        assert observed is not None, "no nonzero FIFO progress observed before slice completion"
        terminal = get_json(f"{base}/slice/progress/{request_id}")
        assert terminal["stage"] == "completed" and terminal["total_percent"] == 100
        headers = {}
        with open(HEADERS, encoding="utf-8") as stream:
            for line in stream:
                if ":" in line:
                    key, value = line.split(":", 1)
                    headers[key.lower()] = value.strip()
        assert headers["x-request-id"] == request_id
        assert headers["content-type"].split(";", 1)[0] == "application/octet-stream"
        assert float(headers["x-print-time-seconds"]) > 0
        assert float(headers["x-filament-used-mm"]) > 0
        assert float(headers["x-filament-used-g"]) > 0
        artifact = open(ARTIFACT, "rb").read()
        assert b"G0 " in artifact or b"G1 " in artifact
        assert b"fixture-gcode" not in artifact
        print(json.dumps({"image": IMAGE, "schema": {"pages": 6, "groups": 41, "options": 342}, "artifact_bytes": len(artifact)}, sort_keys=True))
finally:
    cleanup()
