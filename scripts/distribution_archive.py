"""Build archives with portable relative names and extract only regular files into a fresh directory."""
from contextlib import ExitStack
import gzip
from pathlib import Path, PurePosixPath
import shutil
import tarfile

from distribution import require


def archive(directory, output):
    source, output = Path(directory), Path(output)
    with ExitStack() as stack:
        raw = stack.enter_context(output.open("xb"))
        compressed = stack.enter_context(gzip.GzipFile(filename="", fileobj=raw, mode="wb", mtime=0, compresslevel=5))
        tar = stack.enter_context(tarfile.open(fileobj=compressed, mode="w|"))
        for path in [source, *sorted(source.rglob("*"))]:
            name = path.relative_to(source.parent).as_posix()
            require(not path.is_symlink() and (path.is_file() or path.is_dir()), f"unsupported archive entry: {name}")
            info = tarfile.TarInfo(name)
            if path.is_dir():
                info.type, info.mode = tarfile.DIRTYPE, 0o755
                tar.addfile(info)
            else:
                info.size = path.stat().st_size
                info.mode = 0o755 if path.stat().st_mode & 0o111 else 0o644
                with path.open("rb") as stream:
                    tar.addfile(info, stream)


def extract(archive_path, destination, expected_root):
    destination = Path(destination)
    destination.mkdir(exist_ok=False)
    seen, total = set(), 0
    with tarfile.open(archive_path, "r|*") as tar:
        for member in tar:
            path = PurePosixPath(member.name)
            require(not path.is_absolute() and path.parts and path.parts[0] == expected_root
                    and ".." not in path.parts and "\\" not in member.name and path not in seen
                    and (member.isfile() or member.isdir()), "unsafe or duplicate archive member")
            seen.add(path)
            total += member.size
            require(total < 6 * 1024**3 and len(seen) < 250_000, "archive exceeds distribution limits")
            out = destination.joinpath(*path.parts)
            if member.isdir():
                out.mkdir(parents=True, exist_ok=True)
            else:
                out.parent.mkdir(parents=True, exist_ok=True)
                with tar.extractfile(member) as src, out.open("xb") as dst:
                    shutil.copyfileobj(src, dst)
                out.chmod(0o755 if member.mode & 0o111 else 0o644)
    require(seen and (destination / expected_root).is_dir(), "empty distribution archive")
    return destination / expected_root
