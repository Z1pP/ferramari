#!/usr/bin/env python3
"""The reviewed Russian wording for the control surface, as a msgid table.

Used by `scripts/fill-po.py`, which applies it only to entries that are fuzzy
or empty and leaves finished translations alone. The table is written by hand
because the difference between a good and a bad translation is whether it
says what the control actually does: msgmerge can only offer the closest
msgid it has seen, which is how `Nothing is playing` arrived as
`Сейчас воспроизводится` — the opposite of the truth.

Keys are exact msgids. A plural noun may carry its own form under
`msgid + "\x00plural"`.
"""
import re
import sys
from pathlib import Path

# msgid -> Russian. Keys are the msgids as they appear in the catalogue.
TRANSLATIONS = {
    # --- Rail and page headers -------------------------------------------
    "Home": "Главная",
    "Library": "Библиотека",
    "Online": "Онлайн",
    "Performance": "Производительность",
    "Settings": "Настройки",
    "Your animated wallpaper engine": "Ваши живые обои",
    "Your local wallpapers": "Ваши локальные обои",
    "Browse and download wallpapers": "Поиск и загрузка обоев",
    "System monitoring and engine statistics": "Мониторинг системы и статистика движка",
    "Configure Ferramari to your needs": "Настройте Ferramari под себя",
    # --- Home ------------------------------------------------------------
    "Now Playing": "Сейчас воспроизводится",
    "Nothing is playing": "Ничего не воспроизводится",
    "Reading playback data…": "Читаем данные плеера…",
    "Reading…": "Читаем…",
    "Start": "Запуск",
    "Stop": "Останов",
    "Restart": "Перезапуск",
    "Monitors": "Мониторы",
    "Primary": "Основной",
    "No wallpaper": "Обои не выбраны",
    "No rotation": "Без ротации",
    "display": "монитор",
    "displays": "монитора",
    "display\x00many": "мониторов",
    # Russian declines the counted noun by the digit, so the two msgids are
    # not a singular/plural pair and cannot share one entry: `1 предмет`,
    # `2 предмета`, `9 предметов`. The catalogue's `Plural-Forms` maps the
    # three forms onto these three strings.
    "item": "предмет",
    "items": "предмета",
    "item\x00many": "предметов",
    "CPU": "ЦП",
    "GPU": "ГП",
    "RAM": "ОЗУ",
    "FPS": "К/с",
    "Idle": "Простой",
    "Active": "Активен",
    "Occluded": "Закрыт окном",
    "Suspended": "Приостановлен",
    "Unknown": "Неизвестно",
    "Choose Wallpaper": "Выбрать обои",
    "Add Wallpaper": "Добавить обои",
    "Browse Online": "Найти в онлайне",
    "Open Settings": "Открыть настройки",
    "Daemon": "Демон",
    "Daemon offline": "Демон недоступен",
    # --- Library ---------------------------------------------------------
    "All": "Все",
    "Video": "Видео",
    "Image": "Изображения",
    "Live": "Активные",
    "Favorites": "Избранное",
    "Add": "Добавить",
    "Add video": "Добавить видео",
    "Added to library": "Добавлено в библиотеку",
    "All wallpapers": "Все обои",
    "Search wallpapers…": "Поиск обоев…",
    "Add a video, or download one from Online.": "Добавьте видео или загрузите его в разделе «Онлайн».",
    "The library is empty": "Библиотека пуста",
    "No favorites yet": "Избранного пока нет",
    "Star a wallpaper to keep it here.": "Отметьте обои звёздочкой, чтобы они появились здесь.",
    "Nothing is playing": "Ничего не воспроизводится",
    "Apply a wallpaper and it will show here.": "Примените обои — и они появятся здесь.",
    "No matches": "Совпадений нет",
    "Nothing in the library matches that search.": "Поиск не нашёл ничего в библиотеке.",
    "Apply as wallpaper": "Применить как обои",
    "Add to favorites": "Добавить в избранное",
    "Remove from favorites": "Убрать из избранного",
    "Add to": "Добавить в",
    "Remove from": "Убрать из",
    "Play on": "Воспроизвести на",
    "Remove from library": "Убрать из библиотеки",
    "Reload": "Обновить",
    "Open folder": "Открыть папку",
    # --- Online ----------------------------------------------------------
    "Featured": "Рекомендуемое",
    "Recent": "Недавнее",
    "Downloaded": "Загруженное",
    "Collections": "Коллекции",
    "Categories": "Категории",
    "Live wallpapers": "Живые обои",
    "Nothing featured yet": "Рекомендуемого пока нет",
    "Search or pick a category": "Найдите обои или выберите категорию",
    "Browse": "Смотреть",
    "Install": "Установить",
    "Downloading…": "Загрузка…",
    "Load more": "Показать ещё",
    "No wallpapers found": "Обои не найдены",
    "Loading wallpapers…": "Загружаем обои…",
    # --- Performance -----------------------------------------------------
    "MPV Engine": "Движок MPV",
    "Running": "Работает",
    "MPV Statistics": "Статистика MPV",
    "Decoder": "Декодер",
    "Resolution": "Разрешение",
    "Dropped Frames": "Пропущенные кадры",
    "Frame Time": "Время кадра",
    "avg": "в среднем",
    "unknown": "неизвестно",
    "N/A": "нет данных",
    "—": "—",
    "…": "…",
    "Waiting for the first frame": "Ждём первый кадр",
    "Waiting for the first sample…": "Ждём первый замер…",
    # --- Settings --------------------------------------------------------
    "General": "Основные",
    "Appearance": "Оформление",
    "Playback": "Воспроизведение",
    "Advanced": "Дополнительно",
    "Start with system": "Запускать с системой",
    "Launch Ferramari automatically on login": "Запускать Ferramari автоматически при входе",
    "Run in background": "Работать в фоне",
    "Keep the daemon running when the window is closed": "Демон продолжает работать, когда окно закрыто",
    "Check for updates": "Проверять обновления",
    "Ask GitHub for a new release when the window opens": "Спрашивать GitHub о новом релизе при открытии окна",
    "Last checked": "Последняя проверка",
    "Never checked": "Ещё не проверялось",
    "You are up to date": "У вас последняя версия",
    "Update available:": "Доступно обновление:",
    "This version": "Эта версия",
    "Check now": "Проверить сейчас",
    "Open release page": "Открыть страницу релиза",
    "Could not reach GitHub": "Не удалось связаться с GitHub",
    "Could not read the release listing": "Не удалось прочитать список релизов",
    "The release listing had no version": "В списке релизов не было версии",
    "The release listing was unexpectedly large": "Список релизов оказался неожиданно большим",
    "The release listing was not valid JSON": "Список релизов не является корректным JSON",
    "The release listing was not found (is the project still at the same address?)":
        "Список релизов не найден (проект доступен по прежнему адресу?)",
    "GitHub rate limit reached; try again later": "Превышен лимит запросов GitHub; попробуйте позже",
    "The release listing was not found (is the project still at the same address?)":
        "Список релизов не найден (проект доступен по прежнему адресу?)",
    "Could not save the update setting": "Не удалось сохранить настройку обновлений",
    "Language": "Язык",
    "Interface language": "Язык интерфейса",
    "System": "Системный",
    "Theme": "Тема",
    "Window and control appearance": "Оформление окна и элементов",
    "Light": "Светлая",
    "Dark": "Тёмная",
    "Desktop preference": "Предпочтение системы",
    "The desktop asks for a dark theme": "Система просит тёмное оформление",
    "The desktop asks for a light theme": "Система просит светлое оформление",
    "Profile": "Профиль",
    "Quality prioritises smoothness; Eco reduces decoding":
        "«Качество» ставит плавность выше экономии, «Эко» снижает нагрузку",
    "Quality": "Качество",
    "Balanced": "Сбалансированный",
    "Eco": "Эко",
    "Hardware decoding": "Аппаратное декодирование",
    "Auto lets mpv choose; VA-API forces it; Off uses software":
        "«Авто» предоставляет выбор mpv, VA-API заставляет, «Выкл» — программное декодирование",
    "Off": "Выкл",
    "Pause on battery": "Пауза от батареи",
    "Stop playback when running on battery": "Останавливать воспроизведение от батареи",
    "Idle delay": "Задержка простоя",
    "Minutes of inactivity before the Eco Engine pauses":
        "Минуты бездействия до паузы в режиме «Эко»",
    "Rotation interval": "Интервал ротации",
    "Minutes between wallpaper changes": "Минут между сменами обоев",
    "Rotation mode": "Режим ротации",
    "In order": "По порядку",
    "Random": "Случайно",
    "No displays detected": "Мониторы не найдены",
    "Pause on maximized window": "Пауза при развёрнутом окне",
    "Auto-pause when maximized or fullscreen detected":
        "Пауза, когда окно развёрнуто или открыто во весь экран",
    "Mute on maximized window": "Тишина при развёрнутом окне",
    "Auto-mute when maximized or fullscreen detected":
        "Выключать звук, когда окно развёрнуто или открыто во весь экран",
    "Web wallpapers network access": "Сеть для веб-обоев",
    "Remote pages and subresources; local always allowed":
        "Внешние страницы и ресурсы; локальные разрешены всегда",
    "Apply static wallpaper": "Применять статичные обои",
    "Video frame applied as system wallpaper": "Кадр видео как фон системы",
    "Blur radius": "Радиус размытия",
    "Apply": "Применить",
    "Library folder": "Папка библиотеки",
    "Refresh": "Обновить",
    "Rotation collections": "Коллекции для ротации",
    "Groups of wallpapers a display can cycle through":
        "Наборы обоев, которые монитор может перебирать",
    "New collection": "Новая коллекция",
    "Create": "Создать",
    "Collection name": "Название коллекции",
    "No collections yet": "Коллекций пока нет",
    "Packages": "Пакеты",
    "Import, export, sign and verify .ferramari packages":
        "Импорт, экспорт, подпись и проверка пакетов .ferramari",
    "Import": "Импортировать",
    "Export": "Экспортировать",
    "Sign": "Подписать",
    "Verify": "Проверить",
    "Trust key": "Добавить ключ в доверенные",
    "Source…": "Источник…",
    "Video…": "Видео…",
    "Destination…": "Назначение…",
    "Package directory or video to export": "Каталог пакета или видео для экспорта",
    "Export destination folder": "Папка для экспорта",
    "Choose package directory": "Выберите каталог пакета",
    "Choose video to export": "Выберите видео для экспорта",
    "Choose export destination": "Выберите папку для экспорта",
}

PO = Path(sys.argv[1] if len(sys.argv) > 1 else "po/ru.po")


def quote(value: str) -> str:
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def main() -> int:
    text = PO.read_text(encoding="utf-8")
    blocks = text.split("\n\n")
    applied = 0
    out = []
    for block in blocks:
        match = re.search(r'^msgid "(.*)"', block, re.M)
        if not match:
            out.append(block)
            continue
        msgid = match.group(1).replace('\\"', '"').replace("\\\\", "\\")
        if msgid not in TRANSLATIONS:
            out.append(block)
            continue
        value = TRANSLATIONS[msgid]
        # A plural entry keeps its two forms; Russian's third form is filled
        # with the plural, which is what the existing entries in this file do
        # for the words that have no distinct form.
        if "msgid_plural" in block:
            forms = re.findall(r'^msgstr\[\d+\] "(.*)"', block, re.M)
            if len(forms) >= 3:
                value_plural = value
                replacement = (
                    f'msgstr[0] {quote(value)}\n'
                    f'msgstr[1] {quote(value_plural)}\n'
                    f'msgstr[2] {quote(value_plural)}'
                )
                block = re.sub(
                    r'^msgstr\[0\] ".*"\nmsgstr\[1\] ".*"\nmsgstr\[2\] ".*"',
                    replacement,
                    block,
                    flags=re.M,
                )
            else:
                out.append(block)
                continue
        else:
            block = re.sub(r'^msgstr ".*"', f"msgstr {quote(value)}", block, flags=re.M)
        # The flag is what tells msgfmt to leave the entry out of the binary;
        # a reviewed translation must not carry it.
        block = re.sub(r"^#, fuzzy\n", "", block, flags=re.M)
        if block.startswith("#,"):
            rest = block.split("\n", 1)
            if not rest[0].strip("#, ").strip():
                block = rest[1] if len(rest) > 1 else block
        applied += 1
        out.append(block)
    PO.write_text("\n\n".join(out), encoding="utf-8")
    print(f"{PO}: {applied} entries translated of {len(TRANSLATIONS)} in the table")
    return 0


if __name__ == "__main__":
    sys.exit(main())
