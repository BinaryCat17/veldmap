import os
import subprocess
import hashlib

# --- НАСТРОЙКИ ---
OUTPUT_FILE = "project_context.txt"

# Игнорируем эти файлы/папки даже если их вернул git
BLACKLIST_FILES = {'CMakeCache.txt', 'project_context.txt', 'package-lock.json', '.DS_Store'}
BLACKLIST_DIRS = {'CMakeFiles', '.git', 'build', 'dist', '__pycache__'}

# Разрешенные расширения (код и доки)
ALLOWED_EXTENSIONS = {
    '.c', '.h', '.cpp', '.hpp', '.cc', '.hh',   # C/C++
    '.py', '.sh', '.bat', '.ps1',               # Скрипты
    'Makefile', 'CMakeLists.txt', 'Dockerfile', # Сборка
    '.md', '.txt', '.json', '.yaml', '.yml',    # Конфиги/Доки
    '.qml', '.js', '.rs', '.toml', '.py'                               # UI
}

def get_file_hash(filepath):
    """Считает хеш файла, чтобы найти полные дубликаты по содержимому"""
    hasher = hashlib.md5()
    try:
        with open(filepath, 'rb') as f:
            buf = f.read(65536)
            while len(buf) > 0:
                hasher.update(buf)
                buf = f.read(65536)
        return hasher.hexdigest()
    except:
        return None

def is_allowed(filename):
    if filename in BLACKLIST_FILES: return False
    # Разрешаем точные совпадения (Makefile) или по расширению
    return filename in ALLOWED_EXTENSIONS or \
           any(filename.endswith(ext) for ext in ALLOWED_EXTENSIONS if ext.startswith('.'))

def get_git_files(root_path):
    """Получает список файлов через git ls-files"""
    try:
        # Запускаем git из корня проекта
        result = subprocess.run(
            ['git', 'ls-files', '--cached', '--others', '--exclude-standard'],
            cwd=root_path,
            capture_output=True, text=True, encoding='utf-8'
        )
        if result.returncode != 0: return None
        return result.stdout.splitlines()
    except:
        return None

def create_dump():
    # Определяем корень проекта (родительская папка, если скрипт в tools/)
    script_dir = os.path.dirname(os.path.abspath(__file__))
    # Если скрипт в tools, поднимаемся на уровень выше. Если в корне - остаемся.
    project_root = os.path.dirname(script_dir) if os.path.basename(script_dir) == 'tools' else script_dir
    
    if os.getcwd() != project_root:
        print(f"Меняем рабочую папку на: {project_root}")
        os.chdir(project_root)

    print(f"Анализируем проект: {project_root}")
    
    # 1. Получаем файлы (Git или обычный скан)
    raw_files = get_git_files(project_root)
    if raw_files is None:
        print("Git не найден, сканируем папку вручную...")
        raw_files = []
        for root, dirs, files in os.walk("."):
            dirs[:] = [d for d in dirs if d not in BLACKLIST_DIRS]
            for file in files:
                raw_files.append(os.path.join(root, file))

    # 2. УМНАЯ ФИЛЬТРАЦИЯ (Deduplication)
    unique_paths = {} # {canonical_path: original_relative_path}
    seen_hashes = set()
    
    print("Обработка файлов...")
    for rel_path in raw_files:
        filename = os.path.basename(rel_path)
        
        # Проверка 1: Черный список и расширения
        if not is_allowed(filename): continue
        if any(d in rel_path.split(os.sep) for d in BLACKLIST_DIRS): continue

        # Проверка 2: Канонический путь (Abs + LowerCase)
        full_path = os.path.abspath(rel_path)
        canonical = os.path.normcase(full_path) # c:\proj\file.c == C:\Proj\File.c
        
        if canonical in unique_paths:
            continue # Уже есть этот путь
            
        # Проверка 3: Хеш содержимого (на случай симлинков или копий)
        # file_hash = get_file_hash(full_path)
        # if file_hash and file_hash in seen_hashes:
        #    print(f"  -> Пропуск дубликата по содержимому: {rel_path}")
        #    continue
        # seen_hashes.add(file_hash)

        unique_paths[canonical] = rel_path

    # Сортируем для красоты
    sorted_files = sorted(unique_paths.values())
    print(f"Найдено уникальных полезных файлов: {len(sorted_files)}")

    # 3. ЗАПИСЬ
    with open(OUTPUT_FILE, 'w', encoding='utf-8') as outfile:
        # ДЕРЕВО
        outfile.write(f"PROJECT ROOT: {project_root}\n")
        outfile.write("PROJECT STRUCTURE:\n==================\n")
        last_dir = ""
        for f in sorted_files:
            current_dir = os.path.dirname(f)
            if current_dir != last_dir:
                outfile.write(f"\n[{current_dir if current_dir else 'ROOT'}]\n")
                last_dir = current_dir
            outfile.write(f"  ├── {os.path.basename(f)}\n")

        outfile.write("\n\nFILE CONTENTS:\n==================\n")

        # КОНТЕНТ
        for f in sorted_files:
            if not os.path.exists(f): continue
            
            outfile.write(f"\n\n{'='*60}\nFILE START: {f}\n{'='*60}\n")
            try:
                with open(f, 'r', encoding='utf-8', errors='replace') as source:
                    outfile.write(source.read())
            except Exception as e:
                outfile.write(f"Error reading file: {e}")

    print(f"Готово! Файл {OUTPUT_FILE} создан в корне проекта.")

if __name__ == "__main__":
    create_dump()