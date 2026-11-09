# D&D Table, Rust port

Перенос https://github.com/ArtVemka/dnd-table (Electron + Node + vanilla JS) на Rust. Эталонный JS-проект не
трогаем: он остаётся оракулом поведения, пока паритет не подтверждён тестами.

## Что уже есть

| Часть | Статус |
|---|---|
| Cargo workspace, MSVC-тулчейн | готово |
| Протокол Socket.IO против вендорного клиента 4.8.3 (websocket + polling) | готово |
| Кости и формулы (`dice.js`) | готово, 45 паритет-кейсов из JS |
| Хранилище кампаний (`campaign.js`) | готово, 8 тестов |
| Правила и геометрия (`dnd5e.js`) | готово, 79 тестов |
| Листы, статблоки, приключения (`state.js`) | готово |
| Контракт событий Socket.IO | готово, 130 событий |
| Состояние стола, все 130 событий Socket.IO | готово |
| Оболочка Tauri 2: окно, трей, встроенный сервер, кампании | готово |
| P2P iroh: билет, туннель, QR | готово |
| Discord Rich Presence | готово |
| Open5e и автообновление | не перенесено, кнопки скрыты |
| Android | не трогаем, остаётся WebView-обёрткой |

## Структура

```
crates/dnd-core     модель, правила, кости, хранилище
crates/dnd-server   axum + socketioxide, раздача renderer/
crates/dnd-desktop  Tauri 2: окно, трей, мост window.desktop
crates/dnd-importer импорт архивов Foundry VTT
renderer/           веб-слой, скопирован из E:\DnD без изменений
fixtures/           эталонные данные, снятые с JS-реализации
tools/              генераторы фикстур и паритет-проверки
```

## Десктоп

Данные берутся из `%APPDATA%\dnd-table`, поэтому кампании Electron-версии
открываются сразу. Переопределение: переменная `DND_DATA`.

```powershell
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
cargo run -p dnd-desktop              # окно с рендерером
node tools/desktop-test.js            # полный цикл через живой webview
npx @tauri-apps/cli@2 build           # MSI и NSIS в target\release\bundle
```

Закрытие окна сворачивает приложение в трей; выход — через трей или кнопку
«Выход». Автосохранение по умолчанию раз в минуту, интервал меняется в
настройках.

Звук стола: эмбиент карты и саундборд играют через WebAudio с CORS, громкость
регулируется в меню «Звук» и в настройках, петли переживают сворачивание окна.
Форматы: MP3, OGG, WAV, M4A, WEBM, FLAC, до 32 МБ на файл.

### Портативная сборка

```powershell
powershell -ExecutionPolicy Bypass -File tools/portable.ps1
```

Собирает `dist\dnd-table-portable\` и `dist\DnD-Table-portable.zip`: exe,
`renderer\` и `portable.txt`. Запускать `DnD Table.exe` из папки, установка не
нужна. Кампании, копии и файлы лежат в `data\` рядом с exe, профиль Windows не
трогается. `DND_DATA` по-прежнему переопределяет папку.

## Команды

```powershell
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
cargo test
cargo run -p dnd-server          # печатает LISTENING 127.0.0.1:<port>
node tools/parity-spike.js       # статика + ping/pong двумя транспортами
node tools/gen-dice-fixtures.js  # пересобрать fixtures/dice.json из JS
```

## Правила порта

1. Поведение и русские строки ошибок копируются побайтно: их видит игрок.
2. Каждая перенесённая функция получает эталонный кейс из JS, а не только
   тест на «разумное» поведение.
3. Внешний контракт с `renderer/` держим совместимым: Socket.IO, JSON-поля в
   camelCase, `/files/<name>` для картинок и звука.
4. Старые `campaign.json` формата 2 должны открываться.
