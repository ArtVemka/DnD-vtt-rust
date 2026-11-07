# D&D Table, Rust port

Перенос `E:\DnD` (Electron + Node + vanilla JS) на Rust. Эталонный JS-проект не
трогаем: он остаётся оракулом поведения, пока паритет не подтверждён тестами.

## Что уже есть

| Часть | Статус |
|---|---|
| Cargo workspace, MSVC-тулчейн | готово |
| Протокол Socket.IO против вендорного клиента 4.8.3 (websocket + polling) | готово |
| Кости и формулы (`dice.js`) | готово, 45 паритет-кейсов из JS |
| Хранилище кампаний (`campaign.js`) | готово, 8 тестов |
| Правила и геометрия (`dnd5e.js`) | в работе |
| Листы, статблоки, приключения (`state.js`) | в работе |
| Контракт событий Socket.IO | в работе |
| Состояние стола, `joinRoom`, 140 событий | дальше |
| Оболочка Tauri вместо Electron | дальше |
| Android | не трогаем, остаётся WebView-обёрткой |

## Структура

```
crates/dnd-core     модель, правила, кости, хранилище
crates/dnd-server   axum + socketioxide, раздача renderer/
renderer/           веб-слой, скопирован из E:\DnD без изменений
fixtures/           эталонные данные, снятые с JS-реализации
tools/              генераторы фикстур и паритет-проверки
```

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
