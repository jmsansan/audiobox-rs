#!/usr/bin/env python3
"""Build the static Pages site, real WASM engine and complete Rust API docs."""
import argparse
import json
from html.parser import HTMLParser
from pathlib import Path
import re
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]

def run(*args):
    subprocess.run(args, cwd=ROOT, check=True)

class Snippets(HTMLParser):
    def __init__(self):
        super().__init__()
        self.snippets = []
        self.rust = False
        self.text = []
    def handle_starttag(self, tag, attrs):
        if tag == 'code' and 'language-rust' in dict(attrs).get('class', ''):
            self.rust = True
            self.text = []
    def handle_data(self, data):
        if self.rust:
            self.text.append(data)
    def handle_endtag(self, tag):
        if tag == 'code' and self.rust:
            self.snippets.append(''.join(self.text))
            self.rust = False

class Links(HTMLParser):
    def __init__(self):
        super().__init__()
        self.links = []
        self.ids = set()
    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if 'id' in attrs:
            self.ids.add(attrs['id'])
        for attr in ('href', 'src'):
            if attr in attrs:
                self.links.append(attrs[attr])

def verify_examples():
    snippets = Snippets()
    snippets.feed((ROOT / 'site/docs/index.html').read_text())
    generated = subprocess.run(
        ['node', 'site/tests/collect-examples.mjs'], cwd=ROOT,
        capture_output=True, text=True, check=True)
    snippets.snippets.extend(json.loads(generated.stdout))
    with tempfile.TemporaryDirectory(prefix='audiobox-docs-') as directory:
        project = Path(directory)
        (project / 'examples').mkdir()
        (project / 'Cargo.toml').write_text(
            '[package]\nname = "audiobox-doc-examples"\nversion = "0.0.0"\nedition = "2024"\n'
            f'[dependencies]\naudiobox = {{ path = "{ROOT.as_posix()}" }}\n')
        for i, code in enumerate(snippets.snippets):
            if not re.search(r'\bfn\s+main\s*\(', code):
                code += '\nfn main() {}\n'
            (project / f'examples/guide_{i}.rs').write_text(code)
        run('cargo', 'check', '--offline', '--examples', '--manifest-path', str(project / 'Cargo.toml'))
    print(f'Checked {len(snippets.snippets)} guide, homepage and generated Rust examples.', flush=True)

def verify_links(output):
    documents = {}
    for file in [output / 'index.html', output / 'sandbox.html', output / 'docs/index.html']:
        parsed = Links()
        parsed.feed(file.read_text())
        documents[file.resolve()] = parsed
    for file, parsed in documents.items():
        for link in parsed.links:
            if re.match(r'^(https?:|mailto:|data:)', link):
                continue
            path, _, fragment = link.partition('#')
            target = (file.parent / path).resolve() if path else file
            if target.is_dir():
                target /= 'index.html'
            if not target.exists():
                raise RuntimeError(f'Broken link in {file.name}: {link}')
            if fragment and target in documents and fragment not in documents[target].ids:
                raise RuntimeError(f'Missing fragment in {file.name}: {link}')
    print('Checked all local navigation, asset and API links.', flush=True)

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', default='_site')
    parser.add_argument('--skip-example-check', action='store_true')
    args = parser.parse_args()
    output = (ROOT / args.output).resolve()
    if output == ROOT or ROOT in output.parents and output.name in ('site', 'src'):
        raise RuntimeError('Choose a separate output directory.')
    run('cargo', 'build', '--locked', '--offline', '--release', '--target', 'wasm32-unknown-unknown',
        '--manifest-path', 'site/wasm/Cargo.toml')
    run('cargo', 'doc', '--locked', '--offline', '--no-deps')
    if not args.skip_example_check:
        verify_examples()
    output.mkdir(parents=True, exist_ok=True)
    for relative in ('index.html', 'sandbox.html', '404.html', 'docs/index.html'):
        destination = output / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / 'site' / relative, destination)
    shutil.copytree(ROOT / 'site/assets', output / 'assets', dirs_exist_ok=True)
    shutil.copy2(ROOT / 'site/wasm/target/wasm32-unknown-unknown/release/audiobox_sandbox.wasm',
        output / 'assets/sandbox.wasm')
    shutil.copytree(ROOT / 'target/doc', output / 'api', dirs_exist_ok=True)
    (output / '.nojekyll').touch()
    verify_links(output)
    size = (output / 'assets/sandbox.wasm').stat().st_size
    print(f'Built Pages site at {output}; Rust WASM engine: {size / 1024:.1f} KiB.', flush=True)

if __name__ == '__main__':
    main()
