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
        self.in_rust_code = False
        self.code_buffer = []

    def handle_starttag(self, tag, attrs):
        attrs_dict = dict(attrs)
        self.current_tag = tag
        if tag in self.skip_tags: self.in_skip = True
        
        if tag == 'main' or attrs_dict.get('id') == 'main-content' or 'main-content' in attrs_dict.get('class', ''):
            self.in_main = True
            
        class_val = attrs_dict.get('class', '')
        if self.in_main:
            if tag == 'pre' and 'rust' in class_val:
                self.content.append('\n```rust\n')
                self.in_rust_code = True
            elif tag in ['h3', 'h4'] and 'code-header' in class_val:
                self.content.append('\n```rust\n')
                self.in_rust_code = True

    def handle_endtag(self, tag):
        if tag in self.skip_tags: self.in_skip = False
        if self.in_rust_code:
            if tag == 'pre' or tag in ['h3', 'h4']:
                self.content.append('```\n')
                self.in_rust_code = False
        if tag == 'main': self.in_main = False

    def handle_data(self, data):
        if self.in_main and not self.in_skip:
            if data.strip() == '§' or data.strip() == 'Source' or data.strip() == 'Skip to main content':
                return
            
            if self.in_rust_code:
                # Внутри кода сохраняем всё, включая параметры
                self.content.append(data)
            elif self.current_tag in ['h1', 'h2', 'h3', 'h4']:
                self.content.append(f'\n### {data.strip()}\n')
            else:
                clean = data.strip()
                if clean:
                    self.content.append(clean + ' ')

def parse_file(file_path):
    if not os.path.exists(file_path):
        print(f"Error: File {file_path} not found")
        return
    with open(file_path, 'r', encoding='utf-8') as f:
        html = f.read()
    parser = RustDocParser()
    parser.feed(html)
    
    text = "".join(parser.content)
    # Чистим пустые блоки кода
    text = re.sub(r'```rust\s*```', '', text)
    text = re.sub(r'\n\s*\n', '\n\n', text)
    
    lines = text.split('\n')
    filtered = []
    skip = False
    for line in lines:
        if 'Blanket Implementations' in line or 'Auto Trait Implementations' in line:
            skip = True
        if skip and line.startswith('### ') and 'Implementations' not in line:
            skip = False
        if not skip: filtered.append(line)
    
    print('\n'.join(filtered).strip())

def print_tree(startpath, prefix=''):
    if not os.path.isdir(startpath):
        print(f"Error: {startpath} is not a directory")
        return
    ignored = {'src', 'static.files', 'implementors', 'trait.impl'}
    try:
        items = sorted([d for d in os.listdir(startpath) if d not in ignored and not d.startswith('.')])
    except PermissionError: return
    for i, item in enumerate(items):
        path = os.path.join(startpath, item)
        is_last = (i == len(items) - 1)
        current_prefix = '└── ' if is_last else '├── '
        if os.path.isdir(path):
            print(f"{prefix}{current_prefix}[{item}]")
            print_tree(path, prefix + ('    ' if is_last else '│   '))
        elif item.endswith('.html') and item != 'index.html':
            name = item.replace('.html', '').replace('struct.', 'S: ').replace('trait.', 'T: ').replace('enum.', 'E: ').replace('fn.', 'F: ')
            print(f"{prefix}{current_prefix}{name}")

def find_symbol(root_path, symbol):
    results = []
    for root, dirs, files in os.walk(root_path):
        for f in files:
            if f.endswith('.html'):
                if f == f"struct.{symbol}.html" or f == f"enum.{symbol}.html" or f == f"trait.{symbol}.html" or f == f"type.{symbol}.html" or f == f"fn.{symbol}.html":
                    results.append(os.path.join(root, f))
    if not results:
        for root, dirs, files in os.walk(root_path):
            for f in files:
                if symbol.lower() in f.lower() and f.endswith('.html') and f != 'index.html':
                    results.append(os.path.join(root, f))
    for r in results[:10]:
        print(r)

if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: python doc.py <path> | --tree <dir> | --find <dir> <symbol>")
    elif sys.argv[1] == '--tree':
        print_tree(sys.argv[2] if len(sys.argv) > 2 else '.')
    elif sys.argv[1] == '--find':
        find_symbol(sys.argv[2], sys.argv[3])
    else:
        parse_file(sys.argv[1])
