"""Check the distribution Cargo prepared with cargo publish --dry-run."""
import hashlib
import sys
import tarfile
from pathlib import Path, PurePosixPath


def main():
    archive = Path(sys.argv[1])
    size = archive.stat().st_size
    if size >= 200_000:
        raise SystemExit(f"Package is {size:,} bytes, expected less than 200,000")
    with tarfile.open(archive, "r:gz") as package:
        for member in package:
            if member.isdir():
                continue
            path = PurePosixPath(member.name)
            if not member.isfile() or len(path.parts) < 2 or ".." in path.parts or path.is_absolute():
                raise SystemExit(f"Unexpected archive entry: {path}")
            relative = PurePosixPath(*path.parts[1:])
            if relative.name in {"Cargo.toml", "Cargo.toml.orig", "Cargo.lock", "build.rs",
                                  ".cargo_vcs_info.json", "LICENSE-MIT", "LICENSE-APACHE"}:
                if len(relative.parts) == 1:
                    continue
            if relative.suffix in {".rs", ".md", ".sh", ".py"}:
                continue
            raise SystemExit(f"Unexpected non-source file in package: {relative}")
    print(f"{archive.name}: {size:,} bytes, SHA-256 {hashlib.sha256(archive.read_bytes()).hexdigest()}")


if __name__ == "__main__":
    main()
