# Plotter Controller 0.4.0 (desktop)

Управление перьевым плоттером [Scalable-Pen-Plotter](https://github.com/ubu78/Scalable-Pen-Plotter) через GRBL 1.1 (Arduino Uno). Десктопное приложение: открывается в собственном окне, **без браузера, без HTTP-сервера и открытых портов**. Бэкенд на Rust (tao + wry/WebKitGTK, serialport), интерфейс React/Vite встроен в бинарник (rust-embed), связь UI ↔ Rust — по IPC (схема `plotter://app/`).

## Сборка

Нужен Rust ≥ 1.85 (зависимости на edition 2024) и Node.js ≥ 18. Системные пакеты: `libwebkit2gtk-4.1-dev`, `libgtk-3-dev` (сборка), `libwebkit2gtk-4.1-0` (запуск, Debian/Ubuntu).

```sh
cd ui && npm install && npm run build && cd ..   # интерфейс -> ui/dist (встраивается при компиляции)
cargo build --release                            # окно без localhost
bash appimage/build-appimage.sh                  # release/Plotter-Controller-0.4.0-x86_64.AppImage
```

Пользователь должен быть в группе `dialout` (`sudo usermod -aG dialout $USER`, затем перелогин).

## Самотест (без реальной платы)

Приложение умеет проверять само себя в настоящем окне: конвертеры, сохранение файла, подключение, события.

```sh
python3 scripts/pty_mock.py &          # имитация платы GRBL на паре pty (печатает имя /dev/pts/N)
PLOTTER_SELFTEST=1 PLOTTER_SELFTEST_PORT=/dev/pts/N xvfb-run ./target/release/plotter-controller
# печатает: SELFTEST {"status":true,"svg":true,...} и завершается
```

## Структура

```
src/
  main.rs       окно (tao EventLoop<String> + wry/WebKitGTK), схема plotter://app/, IPC, самотест
  api.rs        единый диспетчер запросов UI: handle(grbl, method, path, body)
  grbl.rs       связь с GRBL: поток ok-подтверждений, опрос «?», шина событий Bus (без сокетов)
  gcode.rs      опции, бумага ≤ A4, эмиссия G-кода (ядро всех конвертеров)
  svg.rs        SVG → полилинии (svgtypes: дуги, H/V, S/T)
  text.rs       текст/Markdown → G-code, Google Fonts (кэш ~/.cache/plotter-controller/fonts), страницы
  skeleton.rs   однолинейные буквы: растр → Zhang–Suen → трассировка → RDP; и PDF-страницы
  doc.rs        DOCX (zip+roxmltree), TXT/MD (utf-8 / cp1251), PDF-текст (pdf-extract)
  pdf.rs        PDF-страница как рисунок: hayro → растр → однолинейные штрихи
ui/src/         React 18 + Vite: ConnectionPanel, ManualPanel, PenPanel, SourcePanel, Preview,
                GcodePanel, SettingsPanel, ConsolePanel; i18n ru/en/et; api.js — единственная точка транспорта
```

## Особенности

- Перо: серво SG90 через выход шпинделя — `M5` вверх, `M3 S255` вниз (настраивается в PenPanel, хранится в localStorage); режим оси Z тоже поддерживается; после команд пера — пауза `G4 P0.15`; в начале G-кода `$32=0`.
- Начало листа — левый верхний угол, X вправо, Y вниз (протяжка бумаги). Ноль по кнопке «0»: `G10 L20 P1 X0 Y0`. Бумага не больше A4 принудительно.
- PDF: страница рендерится как рисунок (hayro, 150 dpi) и утончается до однолинейных штрихов; «как текст» — через pdf-extract; векторного режима нет.
- «Сохранить .gcode» пишет файл через Rust (`~/Downloads` или домашняя папка) — в окне нет браузерных скачиваний.
- Языки интерфейса: русский, English, eesti (переключатель в шапке).
