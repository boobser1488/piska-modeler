#!/usr/bin/env python3
"""Синхронизировать копии частей движка Piska, без которых Piska Modeler не собирается сам.

Piska Modeler живёт в отдельном репозитории и собирается без движка. Из движка взяты
(и помечены как копии):

* `crates/engine-modeler/src/ui/kit.rs` — визуальная система редактора движка: палитра,
  шкалы, шрифты, стиль egui (начало `crates/engine-editor/src/theme.rs` — до раздела
  «Рамки панелей»; виджеты редактора не нужны);
* `crates/engine-modeler/src/ui/controls.rs` — контролы набора (`kit::controls` движка): кнопки,
  поля, списки, флажки одной высоты с рамкой внутри; без частей, которым нужен редактор;
* `crates/engine-modeler/src/ui/boot.rs` — запуск программы и долгие операции (`kit::boot`):
  заставка со знаком, именем и настоящим шагом, фоновая работа с ходом, экран ошибки запуска,
  строка прогресса;
* `crates/engine-modeler/src/ui/metrics.rs` — замер интерфейса (`kit::metrics`) для проверки
  раскладки в тестах;
* `vendor/engine-i18n/` — слой перевода интерфейса и каталог `locale/ru.po`;
* `vendor/engine-build/` — значок в exe на Windows (значок по умолчанию — значок Modeler);
* `vendor/engine-model-formats/` — чтение `.bbmodel`, моделей Minecraft, `.vox`, `.ply`, `.stl`
  и запись `.bbmodel` (тот же код, что у импорта движка).

Запуск (из каталога Modeler):

    python scripts/sync_from_engine.py --engine <путь к движку Piska>

Если Modeler лежит внутри движка подмодулем (`<движок>/modeler`), `--engine` не нужен.
`--only engine-model-formats` (или `kit`, `controls`, `metrics`, `boot`, `engine-i18n`, `engine-build`) обновляет одну копию.
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

PARTS = ("kit", "controls", "metrics", "boot", "engine-i18n", "engine-build", "engine-model-formats")

COPY_HEADER = """//! КОПИЯ `crates/engine-editor/src/{name}.rs` движка Piska — не править руками.
//!
//! Обновить: `python scripts/sync_from_engine.py --engine <движок>`.
#![allow(dead_code, unused_imports)]

"""

# Части `controls.rs`, которым нужен редактор (виджеты темы после «Рамок панелей», окна,
# инспектор): у Modeler их нет — функции с ними не копируются.
EDITOR_ONLY = ("crate::window_kit", "crate::inspector", "theme::segmented", "theme::search_field", "theme::toolbar_strip", "theme::section_label")


def engine_module(engine: Path, name: str) -> str:
    """`controls.rs` / `metrics.rs` движка для Modeler: `crate::theme` → `super::kit`,
    `crate::icons` → `super::icons`, без тестов и без функций, которым нужен редактор."""
    text = (engine / "crates" / "engine-editor" / "src" / f"{name}.rs").read_text(encoding="utf-8")
    text = text.replace("\r\n", "\n")
    cut = text.find("#[cfg(test)]")
    if cut >= 0:
        text = text[:cut]
    lines = []
    for line in text.rstrip().splitlines():
        if line.startswith("//!"):
            line = "//" + line[3:]
        lines.append(line)
    out = []
    i = 0
    while i < len(lines):
        line = lines[i]
        if line.startswith(("pub fn ", "fn ")):
            j = i
            while j < len(lines) and lines[j] != "}":
                j += 1
            body = "\n".join(lines[i : j + 1])
            if any(m in body for m in EDITOR_ONLY):
                while out and out[-1].startswith("///"):
                    out.pop()
                i = j + 1
                continue
        if line.startswith("pub use crate::window_kit"):
            while out and out[-1].startswith("///"):
                out.pop()
            i += 1
            continue
        out.append(line)
        i += 1
    body = "\n".join(out) + "\n"
    body = body.replace("use crate::icons::{self, Icon};", "use super::icons::{self, Icon};")
    body = body.replace("use crate::theme::{\n    self,", "use super::kit as theme;\nuse super::kit::{\n   ")
    body = body.replace("use crate::theme::{self,", "use super::kit as theme;\nuse super::kit::{")
    body = body.replace("use crate::controls;", "use super::controls;")
    body = body.replace("use crate::i18n::", "use engine_i18n::")
    body = body.replace("crate::theme::", "super::kit::")
    bad = [l for l in body.splitlines() if "crate::" in l]
    if bad:
        sys.exit(f"{name}.rs still refers to the editor: {bad[:3]} (update EDITOR_ONLY in this script)")
    return COPY_HEADER.format(name=name) + body


def kit(engine: Path) -> str:
    text = (engine / "crates" / "engine-editor" / "src" / "theme.rs").read_text(encoding="utf-8")
    end = text.rfind("// ----", 0, text.index(MARKER))
    lines = []
    for line in text[:end].rstrip().splitlines():
        if line.startswith("use crate::icons"):
            continue
        if line.startswith("use crate::i18n::"):
            line = line.replace("use crate::i18n::", "use engine_i18n::")  # слой перевода у Modeler — отдельный крейт
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
    parser.add_argument("--only", choices=PARTS, action="append", help="обновить только эти копии (можно несколько раз)")
    args = parser.parse_args()
    parts = set(args.only or PARTS)
    engine = args.engine.resolve()
    if not (engine / "crates" / "engine-editor" / "src" / "theme.rs").is_file():
        sys.exit(f"{engine} is not a Piska Engine checkout (no crates/engine-editor/src/theme.rs); pass --engine")
    if "kit" in parts:
        out = MODELER / "crates" / "engine-modeler" / "src" / "ui" / "kit.rs"
        out.write_text(kit(engine), encoding="utf-8", newline="\n")
        print(f"wrote {out}")
    for name in ("controls", "metrics", "boot"):
        if name in parts:
            out = MODELER / "crates" / "engine-modeler" / "src" / "ui" / f"{name}.rs"
            out.write_text(engine_module(engine, name), encoding="utf-8", newline="\n")
            print(f"wrote {out}")
    if "engine-i18n" in parts:
        copy_crate(engine / "crates" / "engine-i18n", MODELER / "vendor" / "engine-i18n", "Перевод интерфейса и каталог ru.po.")
    if "engine-model-formats" in parts:
        copy_crate(
            engine / "crates" / "engine-model-formats",
            MODELER / "vendor" / "engine-model-formats",
            "Чтение .bbmodel / Minecraft .json / .vox / .ply / .stl и запись .bbmodel.",
        )
    if "engine-build" in parts:
        copy_crate(
            engine / "crates" / "engine-build",
            MODELER / "vendor" / "engine-build",
            "Значок в exe на Windows; значок по умолчанию — значок Piska Modeler.",
            patches=[
                ("src/lib.rs", '"../../../assets/branding/icon.ico"', '"../../../crates/piska-modeler/assets/icon.ico"'),
                ("src/lib.rs", '"../../../assets/branding/icon.png"', '"../../../crates/piska-modeler/assets/icon.png"'),
            ],
        )
    print("vendored: " + ", ".join(sorted(parts)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
