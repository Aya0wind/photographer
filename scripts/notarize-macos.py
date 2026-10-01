"""Submit one DMG once, retain evidence, and staple only explicit acceptance.

Credentials come from the release environment and are never printed. A timeout
does not cancel Apple's processing; use the recorded submission ID to inspect
that submission instead of submitting the same artifact again.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import uuid


def submission_id(result):
    value = result.get("id") if isinstance(result, dict) else None
    try:
        return str(uuid.UUID(value))
    except (ValueError, TypeError, AttributeError):
        return None


def require_accepted(result):
    identity = submission_id(result)
    if not identity or result.get("status") != "Accepted":
        raise ValueError("Notarization did not explicitly return Accepted with a submission ID")
    return identity


def credentials(environment):
    names = ("APPLE_ID", "APPLE_PASSWORD", "APPLE_TEAM_ID")
    if any(not environment.get(name) for name in names):
        raise ValueError("APPLE_ID, APPLE_PASSWORD and APPLE_TEAM_ID are required")
    return ["--apple-id", environment["APPLE_ID"],
            "--password", environment["APPLE_PASSWORD"],
            "--team-id", environment["APPLE_TEAM_ID"]]


def redact(text, environment):
    for key in ("APPLE_PASSWORD", "APPLE_ID"):
        value = environment.get(key)
        if value:
            text = text.replace(value, "[REDACTED]")
    return text


def notarize(dmg, evidence, environment=None, execute=subprocess.run):
    dmg = Path(dmg).resolve(strict=True)
    if not dmg.is_file() or dmg.suffix.lower() != ".dmg":
        raise ValueError("Expected a single existing DMG file")
    environment = os.environ if environment is None else environment
    auth = credentials(environment)
    # Refuse reuse: previous result files must never be mistaken for this run.
    evidence = Path(evidence).absolute()
    evidence.mkdir(parents=True, exist_ok=False)
    digest = hashlib.sha256()
    with dmg.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    (evidence / "artifact.json").write_text(json.dumps({
        "filename": dmg.name, "sha256_before_stapling": digest.hexdigest(),
    }, indent=2))
    response = execute([
        "/usr/bin/xcrun", "notarytool", "submit", str(dmg), *auth,
        "--wait", "--timeout", "30m", "--output-format", "json", "--no-progress",
    ], capture_output=True, text=True, check=False)
    (evidence / "submission.json").write_text(redact(response.stdout or "", environment))
    # Keep diagnostics locally, without echoing credential-bearing command args.
    (evidence / "submission.stderr").write_text(redact(response.stderr or "", environment))
    try:
        result = json.loads(response.stdout)
    except (ValueError, TypeError) as error:
        raise RuntimeError(f"Invalid notarytool response; inspect {evidence}") from error
    identity = submission_id(result)
    if identity:
        (evidence / "submission-id.txt").write_text(identity + "\n")
    try:
        require_accepted(result)
        if response.returncode != 0:
            raise ValueError("notarytool reported a command failure")
    except ValueError as error:
        if identity:
            log = execute([
                "/usr/bin/xcrun", "notarytool", "log", identity, *auth,
                str(evidence / "notary-log.json"),
            ], capture_output=True, text=True, check=False)
            (evidence / "log.stderr").write_text(redact(log.stderr or "", environment))
            log_path = evidence / "notary-log.json"
            if log_path.is_file():
                log_path.write_text(redact(log_path.read_text(), environment))
        raise RuntimeError(
            f"DMG was not accepted; no stapling or resubmission performed. Inspect {evidence}"
        ) from error
    for operation in ("staple", "validate"):
        output = execute(
            ["/usr/bin/xcrun", "stapler", operation, str(dmg)],
            capture_output=True, text=True, check=False,
        )
        (evidence / f"stapler-{operation}.txt").write_text(
            redact((output.stdout or "") + (output.stderr or ""), environment))
        if output.returncode:
            raise RuntimeError(f"stapler {operation} failed; inspect {evidence}")
    return identity


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("dmg")
    parser.add_argument("evidence", help="New evidence directory; must not exist")
    args = parser.parse_args()
    try:
        identity = notarize(args.dmg, args.evidence)
    except (RuntimeError, ValueError, OSError) as error:
        raise SystemExit(str(error))
    print(f"Accepted and stapled: {identity}")
