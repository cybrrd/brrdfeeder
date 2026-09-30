#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Execute the recipes' legal-file packaging in isolated scratch images.

This checks COPY inputs, resulting image documents and vendor labels, not a full
production image build. No compiler, network, container start or publication.
"""
import hashlib
import json
from pathlib import Path
import shlex
import shutil
import subprocess
import tempfile
import uuid

ROOT=Path(__file__).resolve().parents[5]
def call(*args): return subprocess.check_output(args,text=True).strip()
results=[]
for component,folder,recipe in [('brrdfeeder','aviary','engine/Containerfile'),
                                 ('brrdhouse','brrdhouse','Containerfile')]:
    source=ROOT/'Component'/folder
    lines=(source/recipe).read_text().splitlines()
    vendor=next(line.strip().removeprefix('LABEL ').rstrip('\\').strip()
                for line in lines if 'org.opencontainers.image.vendor=' in line)
    assert vendor=='org.opencontainers.image.vendor="cyBRRD Corporation"'
    copy=next(line for line in lines if line.startswith('COPY ') and '/usr/share/doc/'+component+'/' in line)
    args=[a for a in shlex.split(copy)[1:] if not a.startswith('--from=')]
    assert args[-1]=='/usr/share/doc/'+component+'/'
    assert [Path(a).name for a in args[:-1]]==['NOTICE','LICENSE']
    with tempfile.TemporaryDirectory(prefix='ownership-image-') as directory:
        temp=Path(directory)
        for name in ('NOTICE','LICENSE'): shutil.copyfile(source/name,temp/name)
        # The console's /src inputs belong to its build stage, while the engine
        # uses direct context inputs. Both resolve to these byte-identical files.
        (temp/'Containerfile').write_text('FROM scratch\nLABEL '+vendor+'\nCOPY NOTICE LICENSE '+args[-1]+'\n')
        tag='localhost/ownership-packaging:'+component+'-'+uuid.uuid4().hex
        container=None
        image=None
        try:
            subprocess.run(['podman','build','--quiet','--pull=never','--network=none',
                            '--timestamp','0','-t',tag,str(temp)],check=True)
            image=json.loads(call('podman','image','inspect',tag))[0]
            assert image['Config']['Labels']['org.opencontainers.image.vendor']=='cyBRRD Corporation'
            container=call('podman','create','--network=none','--entrypoint','/not-executed',tag)
            hashes={}
            for name in ('NOTICE','LICENSE'):
                subprocess.run(['podman','cp',container+':'+args[-1]+name,str(temp/('image-'+name))],check=True)
                data=(temp/('image-'+name)).read_bytes()
                assert data==(ROOT/name).read_bytes()
                hashes[name]=hashlib.sha256(data).hexdigest()
            results.append(dict(component=component,image_id=image['Id'],source_recipe=str((source/recipe).relative_to(ROOT)),
                                copy_instruction=copy,vendor='cyBRRD Corporation',files=hashes,
                                proof='local packaging only; full production image build not claimed'))
        finally:
            if container: subprocess.run(['podman','rm',container],check=True,stdout=subprocess.DEVNULL)
            if image: subprocess.run(['podman','rmi',image['Id']],check=True,stdout=subprocess.DEVNULL)
print(json.dumps(results,indent=2))
