# Состояние порта

Перенесено:

- `server/state.js`: нормализаторы, `RoomState`, снимки по ролям, токены, карты,
  сцены, инициатива, журнал, склад, календарь, undo.
- `server/socket.js`: все 130 входящих событий в
  `crates/dnd-server/src/handlers/` (sheets, tokens, combat, maps, misc, journal).
- `server/importer`: `crates/dnd-importer`, подключён к десктопу.
- Звук, изображения, сессионный журнал, Discord presence.
- Tauri 2: окно, трей, встроенный сервер, окна листа и журнала, P2P iroh.

Осталось:

- Open5e (бестиарий и заклинания): кнопки скрыты, потому что мост не отдаёт
  `searchOpen5e*` / `previewOpen5e*`.
- Автообновление: `canUpdate: false`, кнопка скрыта.
- Android: остаётся WebView-обёрткой, отдельного порта нет.
