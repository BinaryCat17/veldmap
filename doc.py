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
        
        if tag == 'main' or attrs_dict.get('id') == 'main-content' or 'main-content' in attrs_dict.get('class', ''):
            self.in_main = True
            
        if self.in_main and tag == 'pre' and 'rust' in attrs_dict.get('class', ''):
            self.content.append('\n```rust\n')
            
        if self.in_main and tag in ['h3', 'h4']:
            id_val = attrs_dict.get('id', '')
            if id_val.startswith('variant.') or id_val.startswith('method.') or id_val.startswith('associatedtype.') or id_val.startswith('associatedconstant.'):
                self.content.append('\n#### ')

    def handle_endtag(self, tag):
        if tag in self.skip_tags: self.in_skip = False
        if tag == 'pre' and self.content and self.content[-1].startswith('\n```rust'):
            self.content.append('```\n')
        if tag == 'main': self.in_main = False

    def handle_data(self, data):
        if self.in_main and not self.in_skip:
            # Очистка мусора
            clean_data = data.strip()
            if not clean_data or clean_data == '§' or clean_data == 'Source' or clean_data == 'Skip to main content':
                return
                
            if self.current_tag in ['h1', 'h2', 'h3', 'h4']:
                if self.content and self.content[-1].endswith('#### '):
                    self.content.append(clean_data + '\n')
                else:
                    self.content.append(f'\n### {clean_data}\n')
            else:
                self.content.append(data)

def parse_file(file_path):
    if not os.path.exists(file_path):
        print(f"Error: File {file_path} not found")
        return
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
        if skip_block and line.startswith('### ') and 'Implementations' not in line:
            skip_block = False
        if not skip_block: filtered_lines.append(line)
    
    # Убираем странные склейки в начале
    result = '\n'.join(filtered_lines).strip()
    result = re.sub(r'^.*?(?=(###|```rust))', '', result, flags=re.DOTALL)
    print(result)

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
        print("Usage:")
        print("  python doc.py <path_to_html>")
        print("  python doc.py --tree <dir>")
        print("  python doc.py --find <dir> <symbol>")
    elif sys.argv[1] == '--tree':
        print_tree(sys.argv[2] if len(sys.argv) > 2 else '.')
    elif sys.argv[1] == '--find':
        find_symbol(sys.argv[2], sys.argv[3])
    else:
        parse_file(sys.argv[1])
