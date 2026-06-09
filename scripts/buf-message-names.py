#!/usr/bin/env python3
"""Print the prost-reflect `message_name` type_attribute lines for a buf module.

The buf templates inject `#[derive(prost_reflect::ReflectMessage)]` into every
generated message, and the derive requires each message's fully-qualified
`message_name` explicitly — a hand-maintained but compiler-enforced list in the
module's buf.gen.yaml (ADR-0011). This prints that list, ready to paste, so a
googleapis digest bump or a new local message is a regeneration rather than a
transcription exercise.

Run from a buf module directory (one with buf.yaml), e.g.:

    cd crates/aip-proto         && ../../scripts/buf-message-names.py --skip aip.proto
    cd examples/freight-server  && ../../scripts/buf-message-names.py --only einride.

`--skip` drops a schema-less import-anchor package; `--only` restricts to the
packages the module actually generates (freight extern_path's every `google.*`
package onto aip-proto, so only `einride.*` carries the derive there). The
well-known types are never listed (prost maps them to `prost_types`).
"""

import argparse
import json
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument(
    "--skip",
    action="append",
    default=[],
    metavar="PKG",
    help="package (and its subpackages) to skip, e.g. a schema-less anchor",
)
parser.add_argument(
    "--only",
    action="append",
    default=[],
    metavar="PREFIX",
    help="restrict to packages with this prefix (repeatable)",
)
args = parser.parse_args()

fds = json.loads(
    subprocess.run(
        ["buf", "build", "--as-file-descriptor-set", "-o", "-#format=json"],
        check=True,
        capture_output=True,
    ).stdout
)

names: list[str] = []


def walk(message: dict, prefix: str) -> None:
    full = f"{prefix}.{message['name']}"
    # Synthetic map-entry messages have no generated struct.
    if message.get("options", {}).get("mapEntry"):
        return
    names.append(full)
    for nested in message.get("nestedType", []):
        walk(nested, full)


for file in fds.get("file", []):
    package = file.get("package", "")
    if package.startswith("google.protobuf"):
        continue
    if any(package == s or package.startswith(s + ".") for s in args.skip):
        continue
    if args.only and not any(package.startswith(o) for o in args.only):
        continue
    for message in file.get("messageType", []):
        walk(message, package)

for name in sorted(names):
    print(f"      - 'type_attribute={name}=#[prost_reflect(message_name = \"{name}\")]'")
