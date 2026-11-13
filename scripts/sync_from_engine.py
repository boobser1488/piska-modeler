#!/usr/bin/env python3
"""Синхронизировать копии частей движка Piska, без которых Piska Modeler не собирается сам.

Piska Modeler живёт в отдельном репозитории и собирается без движка. Из движка взяты
(и помечены как копии):

* `crates/engine-modeler/src/ui/kit.rs` — визуальная система редактора движка: палитра,
  шкалы, шрифты, стиль egui (начало `crates/engine-editor/src/theme.rs` — до раздела
  «Рамки панелей»; виджеты редактора не нужны);
* `vendor/engine-i18n/` — слой перевода интерфейса и каталог `locale/ru.po`;
* `vendor/engine-build/` — значок в exe на Windows (значок по умолчанию — значок Modeler).

Запуск (из каталога Modeler):

    python scripts/sync_from_engine.py --engine <путь к движку Piska>

Если Modeler лежит внутри движка подмодулем (`<движок>/modeler`), `--engine` не нужен.
Результат детерминирован; после синхронизации — `cargo test`.
"""

import argparse
import shutil
import sys
from pathlib import Path

MODELER = Path(__file__).resolve().parent.parent

KIT_HEADER = """//! КОПИЯ визуальной системы движка Piska — не править руками.
//!
//! Источник: `crates/engine-editor/src/theme.rs` движка (палитра, шкалы, шрифты, `apply`).
//! Обновить: `python scripts/sync_from_engine.py --engine <движок>`. Так Piska Modeler
//! выглядит как редактор движка и собирается без него.
#![allow(dead_code, unused_imports)]

"""

MARKER = "// Рамки панелей"


def kit(engine: Path) -> str:
    text = (engine / "crates" / "engine-editor" / "src" / "theme.rs").read_text(encoding="utf-8")
    end = text.rfind("// ----", 0, text.index(MARKER))
    lines = []
    for line in text[:end].rstrip().splitlines():
        if line.startswith("use crate::icons"):
            continue
        if line.startswith("//!"):
            line = "//" + line[3:]
        lines.append(line)
    return KIT_HEADER + "\n".join(drop_icon_functions(lines)) + "\n"


def drop_icon_functions(lines):
    """Убрать функции верхнего уровня, рисующие иконки редактора (набор иконок у Modeler
    свой), вместе с doc-комментарием над ними."""
    out = []
    i = 0
    while i < len(lines):
        line = lines[i]
        if line.startswith(("pub fn ", "fn ")):
            j = i
            while j < len(lines) and lines[j] != "}":
                j += 1
            body = "\n".join(lines[i : j + 1])
            if "icons::" in body or "Icon::" in body or "Icon," in body:
                while out and out[-1].startswith("///"):
                    out.pop()
                i = j + 1
                continue
        out.append(line)
        i += 1
    return out


def copy_crate(src: Path, dst: Path, note: str, patches=()):
    if dst.exists():
        shutil.rmtree(dst)
    shutil.copytree(src, dst, ignore=shutil.ignore_patterns("target", "*.meta"))
    (dst / "VENDORED.md").write_text(
        f"Копия `{src.relative_to(src.parent.parent)}` из движка Piska — не править руками.\n"
        f"{note}\nОбновить: `python scripts/sync_from_engine.py --engine <движок>`.\n",
        encoding="utf-8",
        newline="\n",
    )
    for rel, old, new in patches:
        path = dst / rel
        text = path.read_text(encoding="utf-8")
        if old not in text:
            sys.exit(f"cannot patch {path}: `{old}` not found (the engine changed; update this script)")
        path.write_text(text.replace(old, new), encoding="utf-8", newline="\n")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--engine", type=Path, default=MODELER.parent, help="корень движка Piska")
    args = parser.parse_args()
    engine = args.engine.resolve()
    if not (engine / "crates" / "engine-editor" / "src" / "theme.rs").is_file():
        sys.exit(f"{engine} is not a Piska Engine checkout (no crates/engine-editor/src/theme.rs); pass --engine")
    out = MODELER / "crates" / "engine-modeler" / "src" / "ui" / "kit.rs"
    out.write_text(kit(engine), encoding="utf-8", newline="\n")
    print(f"wrote {out}")
    copy_crate(engine / "crates" / "engine-i18n", MODELER / "vendor" / "engine-i18n", "Перевод интерфейса и каталог ru.po.")
    copy_crate(
        engine / "crates" / "engine-build",
        MODELER / "vendor" / "engine-build",
        "Значок в exe на Windows; значок по умолчанию — значок Piska Modeler.",
        patches=[
            ("src/lib.rs", '"../../../assets/branding/icon.ico"', '"../../../crates/piska-modeler/assets/icon.ico"'),
            ("src/lib.rs", '"../../../assets/branding/icon.png"', '"../../../crates/piska-modeler/assets/icon.png"'),
        ],
    )
    print("vendored engine-i18n and engine-build")
    return 0


if __name__ == "__main__":
    sys.exit(main())
