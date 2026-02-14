import sys
import os
import re
from html.parser import HTMLParser

class RustDocParser(HTMLParser):
    def __init__(self):
        super().__init__()
        self.in_main = False
        self.in_skip = False
        self.current_tag = None
        self.content = []
        self.skip_tags = {'script', 'style', 'nav', 'details'}

    def handle_starttag(self, tag, attrs):
        attrs_dict = dict(attrs)
        self.current_tag = tag
        if tag in self.skip_tags: self.in_skip = True
        if tag == 'main' or attrs_dict.get('id') == 'main-content' or attrs_dict.get('class') == 'main-content':
            self.in_main = True
        if self.in_main and tag == 'pre' and 'rust' in attrs_dict.get('class', ''):
            self.content.append('\n```rust\n')

    def handle_endtag(self, tag):
        if tag in self.skip_tags: self.in_skip = False
        if tag == 'pre' and self.content and self.content[-1].startswith('\n```rust'):
            self.content.append('```\n')
        if tag == 'main': self.in_main = False

    def handle_data(self, data):
        if self.in_main and not self.in_skip:
            if data.strip() == '§' or data.strip() == 'Source': return
            if self.current_tag in ['h1', 'h2', 'h3', 'h4']:
                header = data.strip()
                if header: self.content.append(f'\n### {header}\n')
            else: self.content.append(data)

def parse_file(file_path):
    with open(file_path, 'r', encoding='utf-8') as f:
        html = f.read()
    parser = RustDocParser()
    parser.feed(html)
    text = "".join(parser.content)
    text = re.sub(r' +', ' ', text)
    text = re.sub(r'\n\s*\n', '\n\n', text)
    lines = text.split('\n')
    filtered_lines = []
    skip_block = False
    for line in lines:
        if 'Blanket Implementations' in line or 'Auto Trait Implementations' in line:
            skip_block = True
        if skip_block and line.startswith('### '):
            if 'Implementations' not in line: skip_block = False
        if not skip_block: filtered_lines.append(line)
    print('\n'.join(filtered_lines).strip())

def print_tree(startpath, prefix=''):
    if not os.path.isdir(startpath):
        print(f"Error: {startpath} is not a directory")
        return
    ignored = {'src', 'static.files', 'implementors', 'trait.impl', 'help.html', 'settings.html', 'all.html'}
    try:
        items = sorted([d for d in os.listdir(startpath) if d not in ignored and not d.startswith('.')])
    except PermissionError: return
    
    for i, item in enumerate(items):
        path = os.path.join(startpath, item)
        is_last = (i == len(items) - 1)
        current_prefix = '└── ' if is_last else '├── '
        
        if os.path.isdir(path):
            print(f"{prefix}{current_prefix}[{item}]")
            new_prefix = prefix + ('    ' if is_last else '│   ')
            print_tree(path, new_prefix)
        elif item.endswith('.html') and item != 'index.html':
            name = item.replace('.html', '').replace('struct.', 'S: ').replace('trait.', 'T: ').replace('enum.', 'E: ').replace('fn.', 'F: ')
            print(f"{prefix}{current_prefix}{name}")

if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: python doc.py [--tree] <path>")
    elif sys.argv[1] == '--tree':
        print_tree(sys.argv[2] if len(sys.argv) > 2 else '.')
    else:
        parse_file(sys.argv[1])
