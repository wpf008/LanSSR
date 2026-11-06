"""Install matching SPA/PipeWire compile headers without upgrading runtime libs."""
from pathlib import Path, PurePosixPath
import hashlib
import subprocess
import sys
import tarfile

archive = Path(sys.argv[1])
assert hashlib.sha256(archive.read_bytes()).hexdigest() == "c78c95cdff2774418f0a9468e94a1b2800761d322f13104df6501e1a0dc92558"
prefix = Path('/opt/pipewire-headers')
with tarfile.open(archive) as source:
    for member in source.getmembers():
        if not member.isfile():
            continue
        pieces = PurePosixPath(member.name).parts[1:]
        destination = None
        if pieces[:3] == ('spa', 'include', 'spa'):
            destination = prefix / 'include/spa-0.2' / Path(*pieces[2:])
        elif pieces[:2] == ('src', 'pipewire') and pieces[-1].endswith(('.h', '.h.in')):
            destination = prefix / 'include/pipewire-0.3' / Path(*pieces[1:])
        if destination is None:
            continue
        if '..' in pieces:
            raise ValueError('Unsafe header path')
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(source.extractfile(member).read())

version = prefix / 'include/pipewire-0.3/pipewire/version.h.in'
text = version.read_text()
for key, value in {'PIPEWIRE_VERSION_MAJOR':'0', 'PIPEWIRE_VERSION_MINOR':'3', 'PIPEWIRE_VERSION_MICRO':'65', 'PIPEWIRE_API_VERSION':'"0.3"'}.items():
    text = text.replace('@' + key + '@', value)
version.with_suffix('').write_text(text)
multiarch = subprocess.check_output(['dpkg-architecture', '-qDEB_HOST_MULTIARCH'], text=True).strip()
pc = prefix / 'lib/pkgconfig'
pc.mkdir(parents=True, exist_ok=True)
(pc / 'libspa-0.2.pc').write_text(f'''prefix={prefix}
Name: libspa
Description: PipeWire SPA compile headers
Version: 0.2
Cflags: -I${{prefix}}/include/spa-0.2 -D_REENTRANT
''')
(pc / 'libpipewire-0.3.pc').write_text(f'''prefix={prefix}
Name: libpipewire
Description: PipeWire compile headers with Debian runtime
Version: 0.3.65
Requires: libspa-0.2
Libs: -L/usr/lib/{multiarch} -lpipewire-0.3
Cflags: -I${{prefix}}/include/pipewire-0.3 -D_REENTRANT
''')
