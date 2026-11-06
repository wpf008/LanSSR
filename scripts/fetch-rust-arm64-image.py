#!/usr/bin/env python3
"""Download a pinned official Rust ARM64 image and prepare a Docker load archive.

Uses Amazon ECR's mirror of the Docker Official Image, with TLS validation and
SHA256 checks for the selected manifest, config, compressed and expanded layers.
No Docker daemon configuration or registry credentials are changed.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import gzip
import hashlib
import json
import shutil
import subprocess
import tarfile
from pathlib import Path


REGISTRY = "https://public.ecr.aws"
REPOSITORY = "docker/library/rust"
VERSION = "1.88.0-bullseye"
ARM64_MANIFEST = "sha256:60c95b78b164bc809090509235ab00797a07740fe8733b48593cd42de72b5ee1"
ACCEPT = ", ".join([
    "application/vnd.oci.image.index.v1+json",
    "application/vnd.oci.image.manifest.v1+json",
    "application/vnd.docker.distribution.manifest.list.v2+json",
    "application/vnd.docker.distribution.manifest.v2+json",
])


def request(url: str, *, token: str | None = None, output: Path | None = None) -> bytes:
    command = [
        "curl", "--fail", "--location", "--silent", "--show-error",
        "--proto", "=https", "--proto-redir", "=https",
        "--connect-timeout", "15", "--max-time", "1800" if output else "90",
        "--retry", "3", "--retry-delay", "1", "--config", "-",
    ]
    config = "header = " + json.dumps("Accept: " + ACCEPT) + "\n"
    if token:
        config += "header = " + json.dumps("Authorization: Bearer " + token) + "\n"
    if output:
        command += ["--output", str(output)]
    command.append(url)
    result = subprocess.run(command, input=config.encode(), capture_output=True)
    if result.returncode:
        # curl's terse errors do not print authorization headers or signed URLs.
        raise RuntimeError(result.stderr.decode(errors="replace").strip())
    return result.stdout


def digest_file(path: Path) -> str:
    result = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            result.update(chunk)
    return "sha256:" + result.hexdigest()


def verify_bytes(data: bytes, expected: str) -> None:
    actual = "sha256:" + hashlib.sha256(data).hexdigest()
    if actual != expected:
        raise RuntimeError(f"Digest mismatch: expected {expected}, received {actual}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path,
                        default=Path("target/build-tools/rust-arm64-image.tar"))
    parser.add_argument("--jobs", type=int, default=3)
    parser.add_argument("--tag", default=f"public.ecr.aws/{REPOSITORY}:{VERSION}")
    args = parser.parse_args()
    if not 1 <= args.jobs <= 5:
        parser.error("--jobs must be between 1 and 5")
    if shutil.which("curl") is None:
        parser.error("curl is required")
    output = args.output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    cache = output.parent / "rust-arm64-image-cache"
    cache.mkdir(exist_ok=True)

    auth = json.loads(request(REGISTRY + "/token/?service=public.ecr.aws&scope=aws"))
    token = auth["token"]
    index = json.loads(request(f"{REGISTRY}/v2/{REPOSITORY}/manifests/{VERSION}", token=token))
    selected = [m for m in index.get("manifests", [])
                if m.get("platform", {}).get("architecture") == "arm64"
                and m.get("platform", {}).get("os") == "linux"]
    if len(selected) != 1 or selected[0]["digest"] != ARM64_MANIFEST:
        raise RuntimeError("The official tag no longer matches the pinned ARM64 manifest")
    manifest_data = request(f"{REGISTRY}/v2/{REPOSITORY}/manifests/{ARM64_MANIFEST}", token=token)
    verify_bytes(manifest_data, ARM64_MANIFEST)
    manifest = json.loads(manifest_data)
    config_digest = manifest["config"]["digest"]
    config_data = request(f"{REGISTRY}/v2/{REPOSITORY}/blobs/{config_digest}", token=token)
    verify_bytes(config_data, config_digest)
    config = json.loads(config_data)
    if config.get("architecture") != "arm64" or config.get("os") != "linux":
        raise RuntimeError("The pinned image config is not Linux ARM64")
    layers = manifest["layers"]
    diff_ids = config["rootfs"]["diff_ids"]
    if len(diff_ids) != len(layers):
        raise RuntimeError("Config and manifest have different layer counts")
    print(f"Verified official Rust {VERSION}: {len(layers)} ARM64 layers, "
          f"{sum(layer['size'] for layer in layers) / 1024**2:.1f} MiB compressed", flush=True)

    def download_layer(item: tuple[int, dict]) -> Path:
        number, layer = item
        digest = layer["digest"]
        if not digest.startswith("sha256:") or len(digest) != 71:
            raise RuntimeError("Unsupported layer digest")
        archive = cache / (digest.split(":", 1)[1] + ".tar.gz")
        expanded = cache / (digest.split(":", 1)[1] + ".tar")
        if not archive.exists() or archive.stat().st_size != layer["size"] or digest_file(archive) != digest:
            print(f"Downloading layer {number + 1}/{len(layers)} "
                  f"({layer['size'] / 1024**2:.1f} MiB)", flush=True)
            partial = archive.with_suffix(archive.suffix + ".part")
            request(f"{REGISTRY}/v2/{REPOSITORY}/blobs/{digest}", token=token, output=partial)
            if partial.stat().st_size != layer["size"] or digest_file(partial) != digest:
                raise RuntimeError(f"Layer {number + 1} size or digest mismatch")
            partial.replace(archive)
        if not expanded.exists() or digest_file(expanded) != diff_ids[number]:
            partial = expanded.with_suffix(expanded.suffix + ".part")
            with gzip.open(archive, "rb") as compressed, partial.open("wb") as destination:
                shutil.copyfileobj(compressed, destination, length=1024 * 1024)
            if digest_file(partial) != diff_ids[number]:
                raise RuntimeError(f"Expanded layer {number + 1} digest mismatch")
            partial.replace(expanded)
        print(f"Verified layer {number + 1}/{len(layers)}", flush=True)
        return expanded

    with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as executor:
        expanded_layers = list(executor.map(download_layer, enumerate(layers)))
    config_name = config_digest.split(":", 1)[1] + ".json"
    config_path = cache / config_name
    config_path.write_bytes(config_data)
    load_manifest = [{"Config": config_name, "RepoTags": [args.tag],
                      "Layers": [str(layer.stem) + "/layer.tar" for layer in expanded_layers]}]
    manifest_path = cache / "manifest.json"
    manifest_path.write_text(json.dumps(load_manifest), encoding="utf-8")
    partial_tar = output.with_suffix(output.suffix + ".part")
    with tarfile.open(partial_tar, "w") as archive:
        archive.add(config_path, arcname=config_name, recursive=False)
        archive.add(manifest_path, arcname="manifest.json", recursive=False)
        for layer in expanded_layers:
            archive.add(layer, arcname=layer.stem + "/layer.tar", recursive=False)
    partial_tar.replace(output)
    print(f"Docker load archive: {output}", flush=True)
    print(f"Import using: docker load --input {output}", flush=True)


if __name__ == "__main__":
    main()
