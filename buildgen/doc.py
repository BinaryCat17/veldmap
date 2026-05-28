import sys
import os
import re
from html.parser import HTMLParser

class RustDocParser(HTMLParser):
    def __init__(self):
        super().__init__()
        self.content = []
        self.in_main = False
        self.in_code = False
        self.skip_depth = 0
        self.current_tag = None

    def handle_starttag(self, tag, attrs):
        self.current_tag = tag
        attrs_dict = dict(attrs)
        cls = attrs_dict.get('class', '')
        tag_id = attrs_dict.get('id', '')

        # Входим в основной контент
        if tag == 'main' or tag_id == 'main-content':
            self.in_main = True
        
        if not self.in_main: return

        # Пропускаем навигацию и лишние блоки
        if 'sidebar' in cls or tag in ['script', 'style', 'nav']:
            self.skip_depth += 1
            return

        # Заголовки
        if tag in ['h1', 'h2', 'h3', 'h4']:
            self.content.append(f'\n\n{"#" * int(tag[1:])} ')

        # Блоки кода (сигнатуры, методы, поля)
        # В новом Rustdoc это часто .item-decl, .code-header или просто <code> внутри <section>
        if tag == 'pre' or 'code-header' in cls or 'item-decl' in cls or tag_id.startswith('method.') or tag_id.startswith('structfield.'):
            if not self.in_code:
                self.content.append('\n```rust\n')
                self.in_code = True

    def handle_endtag(self, tag):
        if not self.in_main: return
        
        if tag == 'main':
            self.in_main = False
        
        if self.skip_depth > 0:
            if tag in ['script', 'style', 'nav'] or tag == 'div': # приближенно
                self.skip_depth -= 1
            return

        if self.in_code:
            # Закрываем блок кода при выходе из ключевых тегов
            if tag in ['pre', 'section', 'h3', 'h4', 'code']:
                self.content.append('```\n')
                self.in_code = False

    def handle_data(self, data):
        if not self.in_main or self.skip_depth > 0: return
        
        clean = data.replace('§', '').strip()
        if not clean: return
        
        if self.in_code:
            self.content.append(data)
        else:
            self.content.append(clean + ' ')

def parse_file(file_path):
    if not os.path.exists(file_path): return
    with open(file_path, 'r', encoding='utf-8') as f:
        html = f.read()
    parser = RustDocParser()
    parser.feed(html)
    text = "".join(parser.content)
    # Финальная чистка
    text = re.sub(r'```rust\s*```', '', text)
    text = re.sub(r'\n\s*\n+', '\n\n', text)
    print(text.strip())

def find_symbol(root_path, symbol):
    print(f"Searching for '{symbol}'...")
    matches = []
    for root, _, files in os.walk(root_path):
        for f in files:
            if f.endswith('.html') and symbol.lower() in f.lower():
                matches.append(os.path.join(root, f))
    
    for m in matches[:3]:
        print(f"\n--- {m} ---")
        parse_file(m)

if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: python doc.py <file> | --find <dir> <symbol> | --tree <dir>")
    elif sys.argv[1] == '--find': find_symbol(sys.argv[2], sys.argv[3])
    elif sys.argv[1] == '--tree':
        for root, dirs, files in os.walk(sys.argv[2]):
            if any(x in root for x in ['static.files', 'src']): continue
            level = root.replace(sys.argv[2], '').count(os.sep)
            indent = ' ' * 4 * level
            print(f"{indent}{os.path.basename(root)}/")
            for f in sorted(files):
                if f.endswith('.html') and f != 'index.html':
                    print(f"{indent}    {f}")
    else: parse_file(sys.argv[1])
