# Контракт реального времени: socket.io стол

Источник: `E:\DnD\server\socket.js` (4366 строк), `E:\DnD\server\state.js`.
Всё, что ниже, снято с кода, а не с интерфейса. Русские строки ошибок — дословно, побайтно, включая `ё`, `«»`, тире и многоточия.

## 0. Транспорт и точка входа

`createTableServer({ room, store, hostKey, preferredPort = 32100, onDirty, onLog })` → `{ port, localUrl, io, httpServer, close() }`.

- HTTP (Express, `x-powered-by` выключен):
  - `GET /` → `text/plain; charset=utf-8`: `Стол D&D «<название>» работает.\nПодключайтесь через приложение.\n`
  - `GET /api/info` → `{ ok: true, campaign: { id, name }, players: <число записей> }`
  - `GET /files/:name` → файл из папки кампании, `Access-Control-Allow-Origin: *`. Имя санитизируется стором (basename), выход за папку отбрасывается; нет кампании → 404; битый путь → 400.
- Socket.IO: `cors.origin = '*'`, `maxHttpBufferSize = 20 * 1024 * 1024`.
- Слушает только `127.0.0.1`; при занятом порту `findFreePort(preferred + 1)`.
- `close()`: `io.disconnectSockets(true)`, `io.close()`, страховка 5 с, снимает таймеры hp-уведомлений.

### Комнаты

| Комната | Кто в ней |
|---|---|
| `table` | все, кто успешно сделал `joinRoom` |
| `dm-sockets` | сокеты с `socket.data.role === 'dm'` |
| `dm` (`DM_PLAYER_ID`) | сокет мастера; комната с именем его playerId |
| `<playerId>` | все окна одного игрока (мультиокно) |

Важно: `io.emit(...)` бьёт по **всем подключённым сокетам**, включая те, что ещё не сделали `joinRoom`. `io.to('table')` — только присоединившиеся. `io.except('dm-sockets')` — все, кроме мастеров, включая не присоединившихся.

### Соглашения об ответах

- `ack` необязателен: `const respond = typeof ack === 'function' ? ack : () => {}`. Часть событий его не принимает вовсе.
- Типовой успех: `{ ok: true, ... }`. Типовой отказ: `{ ok: false, error: '<русский текст>' }`.
- Отказ по личности: `{ ok: false, code: 'identity-denied', error: '...' }`.
- Отдельный канал ошибок: `socket.emit('errorMessage', { message })`. Часть обработчиков шлёт **и** ack, **и** `errorMessage`, часть — только одно, часть молча `return`. См. таблицы.
- `requireJoined`: если `socket.data.role` пуст → `errorMessage` `Сначала подключитесь к столу`, `return false`.
- `requireDm`: `requireJoined`, затем `role !== 'dm'` → `errorMessage` `Это действие доступно только ДМ`, `return false`.

---

## 1. Общая модель

### 1.1 Подключение

1. Сокет создаётся, `socket.data = { role: null, playerId: null, name: '' }`. Никаких данных до `joinRoom` не отдаётся, но широковещательные `io.emit` до него доходят.
2. Клиент шлёт `joinRoom` c `{ name, hostKey?, playerId?, playerKey? }`.
3. Имя: `sanitizeName` = `sanitizeText(raw, 40)` + схлопывание `\s+` в пробел + `trim`. Пустое → отказ.
4. Роль:
   - **dm**: `isLoopbackAddress(socket.handshake.address) && safeEqual(data.hostKey, hostKey)`. `safeEqual` — `crypto.timingSafeEqual`, длины должны совпасть и быть > 0.
   - иначе **player**.
5. DM: `room.addDm(name)` (запись `dm` одна на стол), `players.get('dm').sockets.add(id)`, `online = true`, `socket.join('dm')`.
6. Игрок:
   - `requested === 'dm'` → `identity-denied`, `Это код мастера. Войдём заново под своим кодом.`, выхода нет.
   - `playerId` валиден по `/^[A-Za-z0-9_-]{4,64}$/`. Невалидный → выдаётся `p-<uuid>`.
   - существующий игрок с `playerKey`: ключ обязан совпасть (`identity-denied`, `Игрок с таким кодом уже за столом. Войдём заново под новым кодом.`).
   - существующий без ключа, но `online` → тот же отказ.
   - иначе `playerId = wanted || p-<uuid>`, новый `playerKey = 32 hex` (24 случайных байта).
   - `room.addPlayer(playerId, name, playerKey)`: при первом появлении создаётся пустой лист (`createEmptySheet(name)`), при возврате обновляется имя, а ключ закрепляется, если его не было.
7. Сокет входит в `table`, при DM ещё в `dm-sockets`, игрок — в комнату своего `playerId`.
8. `socket.emit('joined', { you: { playerId, name, role, playerKey }, state: room.snapshotFor({ playerId, role }) })`, затем `ack({ ok: true, you })` (без `state`), `markDirty()`, `broadcastPlayers()`, строка в лог.

### 1.2 Роли и права

| Право | dm | player | не joined |
|---|---|---|---|
| `joinRoom`, `ping` | + | + | + |
| Чтение своего листа (`requestCharacterSheet`) | любой лист | только свой | − |
| Правка листа, каст, спасброски смерти | − | только свой | − |
| Всё про карты, туман, стены, свет, шаблоны, пинги карты | + | `mapPing` | − |
| Фишки: создание/удаление/скрытие/статы/изображение | + | `moveToken` своей, `setTokenImage` своей, `setTokenEmote` своей | − |
| Атака из листа | − | + | − |
| Атака монстра, урон, AoE, стабилизация | + | − | − |
| Инициатива, режим боя | + | − | − |
| Броски: `rollDice`, `rollTable` | + (включая `hidden`/`dmOnly`) | + (без `hidden`) | − |
| Чат | + | + (шёпот только мастеру) | − |
| Журнал, склад, календарь, время, сцены, опросы, XP, импорт SRD, undo | + | `setPartyNotes`, `voteCast` | − |
| Звук: загрузка/удаление/эмбиент | + | `playSound` (!) | − |

### 1.3 `requireJoined` и побочные эффекты

Почти каждый мутирующий обработчик: проверка → мутация → `markDirty()` → рассылка. Порядок «сначала ДМ, потом игроки» соблюдается в `emitToken`, `broadcastMapUpdate`, `broadcastPlayers`, `broadcastJournal`, `emitVote`, `rollDice`.

`markDirty()` = `onDirty()`. В main-процессе это `dirty = true; dirtyVersion += 1`; автосейв раз в `autosaveIntervalMs` (по умолчанию 60 000 мс, допустимо 5 000…3 600 000) вызывает `room.serialize()` → `store.save()` → `sweepFiles()`. Сохранения по событию нет: падение процесса между тиками теряет всё с прошлого тика.

### 1.4 snapshotFor(viewer)

Единый снимок состояния, уходит в `joined` и `stateReload`.

```
{
  campaign: { id, name, createdAt, updatedAt, version },
  adventure: isDm ? adventure : null,
  players: playerList({ includeTokens: true, includeDmNotes: isDm, includeHidden: isDm }),
  sheets: isDm ? все : { [свой playerId]: лист },
  maps: isDm ? maps : mapsForViewer (только published),
  activeMap: isDm ? activeMap : (activeMapId ? activeMap : null),
  activeMapId,
  scenes: isDm ? sceneSummary[] : [],
  grid, gameTime, timer, calendarMonths, calendarEvents, partyNotes,
  tokens: isDm ? все : только isTokenVisibleToPlayers, каждый через tokenForViewer,
  initiative, combat,
  diceHistory: isDm ? как есть : hideRollResult,
  chatHistory: без шёпота чужих,
  journal: isDm ? строка : '',
  journalEntries: isDm ? все : visible,
  inventory, inventoryCoins,
  rollRequests: isDm ? все : { !to || to === свой },
  soundboard: isDm ? все : [],
  reactions: isDm ? все : свои
}
```

`playerList` (элемент): `{ playerId, name, role, avatar, online, hasSheet, dmNote (только ДМ), tokenId }`, сортировка: ДМ → online раньше offline → имя `localeCompare`.

`tokenForViewer(token, isDm)`:
- если у фишки есть владелец с листом → подменяет `maxHp`/`hp` значениями листа (`effectiveMaxHp`, `clampInt(sheet.hp, 0, maxHp, 0)`);
- игроку: если `side === 'enemy'` (или `side` пуст и нет владельца) **и** `maxHp` истинно → `maxHp = null, hp = null, armorClass = null`;
- `hideName` → `name = ''`, `nameHidden = true`.

`isTokenVisibleToPlayers(token)`: `!hidden` **и** карта опубликована (`mapId` пуст → «во вкладке есть хоть одна published»).

---

## 2. Входящие события

Обозначения получателей в колонке «Исходящие»:

- `S` — только отправителю; `IO` — `io.emit` (все сокеты); `T` — `table`; `P` — `table` без `dm-sockets`; `D` — `dm-sockets`; `R(x)` — комната `playerId = x`.
- `→ev` — событие, `{...}` — его payload.

Колонка «Кто»: `dm`, `player`, `joined` (любая роль), `any` (без проверки).

В текстах ошибок `<...>` — подстановка из кода (`<имя>`, `<N>`, `<состояние>`); всё остальное, включая `«»`, `ё` и тире, — дословно.

### 2.1 Подключение и служебное

| Событие | payload | Кто | Мутации | Исходящие | Ошибки (дословно) |
|---|---|---|---|---|---|
| `joinRoom` | `{ name, hostKey?, playerId?, playerKey? }`, ack `{ok, you}` / `{ok:false, code?, error}` | any | players, sheets (пустой лист новому), playerKey, online, sockets | `S→joined {you,state}`, `T→playersUpdate`, `D→playersUpdate`, ack | `Имя не может быть пустым` (ack без `code` + `errorMessage`); `Это код мастера. Войдём заново под своим кодом.`; `Игрок с таким кодом уже за столом. Войдём заново под новым кодом.` (оба с `code:'identity-denied'`, только ack) |
| `ping` | `{ echo? }` | any | нет | `S→pong {time, echo}` | — |
| `disconnect` | — | any | `players[x].sockets`, `online` | `playersUpdate` | — |

### 2.2 Листы персонажа

| Событие | payload | Кто | Мутации | Исходящие | Ошибки |
|---|---|---|---|---|---|
| `saveCharacterSheet` | `{ sheet }` | player | `sheets[playerId] = normalizeSheet(...)`; `syncSheetTokens` | `R(playerId)→characterSheetData {playerId,sheet}`; `D→characterSheetUpdated`; по каждой фишке `emitToken('tokenUpdated')`; при первом листе `playersUpdate` | `ДМ не редактирует листы персонажей` |
| `castSpell` | `{ spellId, slotLevel? }` | player | `sheet.spellSlotsUsed[level-1] += 1`, если `level > 0` | `IO→chatMessage`, `R(playerId)→characterSheetData`, `D→characterSheetUpdated` | `Каст заклинания идёт от лица игрока`; `Лист персонажа ещё не готов`; `Заклинание не найдено в листе`; `Нет свободной ячейки <N> уровня`; `Нет свободной ячейки` (недостижима) |
| `rollDeathSave` | — | player | `sheet.hp`, `deathSavesSuccesses/Failures`; `diceHistory` | `R→characterSheetData`, `D→characterSheetUpdated`, `tokenUpdated` по фишкам, `IO→diceRollResult` | `Спасбросок смерти бросает сам игрок`; `Лист персонажа ещё не готов`; `Спасброски смерти идут при нулевом здоровье` |
| `resetDeathSaves` | — | player | обнуляет счётчики | `R→characterSheetData`, `D→characterSheetUpdated` | `Счётчики спасбросков сбрасывает игрок`; `Лист персонажа ещё не готов` |
| `requestCharacterSheet` | `{ playerId }` | joined | нет | `S→characterSheetData {playerId,sheet}` | `Можно смотреть только свой лист персонажа`; `Лист персонажа не найден` |

Правила спасброска смерти: `crypto.randomInt(1,21)`; при истощении с помехой берётся `min` из двух; 20 → `hp = 1`, счётчики 0; 1 → +2 провала; ≥10 → +1 успех; иначе +1 провал; всё зажато `maxDeathSaves = 3`.

### 2.3 Участники и аватары

| Событие | payload | Кто | Мутации | Исходящие | Ошибки |
|---|---|---|---|---|---|
| `setPlayerName` | `{ name }` | joined | `players[x].name`, `sheet.playerName` если совпадало со старым | `playersUpdate` (игрокам и ДМ) | `Имя не может быть пустым`; `Участник не найден` |
| `setAvatar` | `{ image: dataURL \| null \| '' }`, async | joined | `players[x].avatar`; у фишек с `avatarFromSheet` — `imageUrl`, сброс кадрирования | `playersUpdate`, `tokenUpdated` по фишкам | ошибки `decodeImage`; `Не получилось сохранить аватар` / текст квоты |

### 2.4 Токены

Общий механизм: `emitToken(event, token, payload?)` шлёт `D` полную версию (`tokenForViewer(token, true)`), `P` — либо присланный `payload`, либо `tokenForViewer(token, false)`. Если `!isTokenVisibleToPlayers(token)` — `P` не получает ничего.

| Событие | payload | Кто | Мутации | Исходящие | Ошибки |
|---|---|---|---|---|---|
| `createTokenFromSheet` | `{ playerId, mapType?, mapId? }` | dm | push скрытой фишки (`side:'ally'`, `avatarFromSheet:true`, hp из листа) | `tokenCreated` (только D, фишка hidden), `playersUpdate` | `Игрок не найден`; `У мастера нет фишки на поле`; `У игрока уже есть фишка «<имя>»`; `Слишком много фишек на столе` |
| `createToken` | `{ name?, mapType?, mapId?, side?, attackBonus?, armorClass?, size?, maxHp? }` | dm | push скрытой фишки | `tokenCreated` (только D) | `Слишком много фишек на столе` |
| `setTokenHp` | `{ tokenId, hp }` | dm | `setTokenHp` (лист владельца при наличии), `pushUndo` при отличии | `tokenUpdated`, при листе `R→characterSheetData` + `D→characterSheetUpdated`, отложенное chat-уведомление | `Фишка не найдена`; `У фишки «<имя>» нет прочности` |
| `applyTokenDamage` | `{ tokenId, amount, type?, healing? }` | dm | `applyTokenDamage` → hp/лист/временные хиты/смерть/концентрация | `tokenUpdated`*, `characterSheetData/Updated`, chat-уведомление, `rollRequestUpdated` | `Фишка не найдена`; `Укажите количество урона или лечения`; `У фишки «<имя>» нет прочности` |
| `attackToken` | `{ tokenId, bonus, damage, name? }` | player | бросок d20+bonus против КД; при попадании `applyTokenDamage` | `IO→diceRollResult`, далее как у урона | `Быстрая атака идёт от листа игрока`; `Лист ещё не готов`; `Фишка не найдена`; `КД цели неизвестен`; ошибки `parseFormula` |
| `monsterAttack` | `{ tokenId, bonus, damage, name? }` | dm | то же, без листа нападающего | `IO→diceRollResult`, далее как у урона | `Фишка не найдена`; `КД цели неизвестен`; ошибки `parseFormula` |
| `castAoe` | `{ type, mapId, templateId, save, dc, damage, damageType?, halfOnSave?, spellName?, tokenIds[] }` | dm | заявки игрокам; урон монстрам с бросанием спасброска сервером | `rollRequestUpdated` (если есть игроки), `tokenUpdated`, `IO→diceRollResult` на каждую фишку без владельца | `Карта не найдена`; `Шаблон не найден`; `Укажите характеристику спасброска`; `Сложность спасброска от 1 до 40`; `В шаблоне нет фишек для урона`; `Накрытые фишки теперь на другой карте или без прочности`; ошибки формулы |
| `setTokenSize` | `{ tokenId, size 1..4 }` | dm | `size` | `tokenUpdated` | `Фишка не найдена` |
| `setTokenSide` | `{ tokenId, side: 'ally'\|'enemy' }` | dm | `side` | `tokenUpdated` | `Фишка не найдена`; `Неизвестная сторона фишки` |
| `setTokenAttackBonus` | `{ tokenId, attackBonus -5..20 }` | dm | `attackBonus` | `tokenUpdated` | `Фишка не найдена`; `Бонус атаки игрока берётся из его листа` |
| `setTokenArmorClass` | `{ tokenId, armorClass }` (`null`/`''`/`0` → `null`) | dm | `armorClass` | `tokenUpdated` | `Фишка не найдена`; `Класс брони игрока берётся из его листа` |
| `setTokenConcentration` | `{ tokenId, concentration }` | dm | `concentration` (пусто → `undefined`) | `tokenUpdated` | `Фишка не найдена`; `Концентрация героя живёт в его листе` |
| `setTokenEmote` | `{ tokenId, text }` | joined (dm или владелец) | `emote = { text, until: now+6000 }`, таймер на 6200 мс снимает | `tokenUpdated` | `Фишка не найдена`; `Эмоут ставит владелец фишки или мастер` |
| `requestStabilize` | `{ tokenId }` | dm | `addRollRequest` (Медицина, СЛ 10) | `rollRequestUpdated` | `Фишка не найдена`; `Стабилизируют только фишки героев`; `Лист героя ещё не готов` |
| `setTokenHidden` | `{ tokenId, hidden? }` (`hidden === false` — показать, иначе скрыть) | dm | `token.hidden` | скрытие: `IO→tokenHidden {tokenId,hidden:true}`; показ: `D→tokenHidden {hidden:false}` + `tokenCreated` (если была скрыта) | `Фишка не найдена` |
| `tokensBatch` | `{ actions: [{ kind:'hidden'\|'delete'\|'side', tokenId, value? }] }`, до 200 | dm | скрытие/сторона/удаление; `pushUndo` один на пачку; удаление выкидывает из инициативы | `tokenHidden`/`tokenCreated`/`tokenUpdated`/`tokenDeleted` по каждой, `initiativeUpdated`, `playersUpdate` | `Нет фишек для групповой операции` |
| `setAllTokensHidden` | `{ hidden?, side?: 'ally'\|'enemy' }` | dm | скрывает/показывает все (или только сторону) | на каждую фишку свои `tokenHidden`/`tokenCreated`, **и** `playersUpdate` на каждую | — |
| `setTokenNameHidden` | `{ tokenId, hidden? }` | dm | `hideName` | `tokenUpdated` | `Фишка не найдена` |
| `setTokenLabelsHidden` | `{ tokenId, hidden? }` | dm | `hideLabels` | `tokenUpdated` | `Фишка не найдена` |
| `renameToken` | `{ tokenId, name }` | dm | `name` | `tokenUpdated` | `Фишка не найдена`; `Имя фишки не может быть пустым` |
| `assignTokenToPlayer` | `{ tokenId, playerId \| null }` | dm | `assignedPlayerId`, при назначении `side='ally'`, `armorClass=null`, `attackBonus=0`, `syncSheetTokens` | `tokenUpdated`, `playersUpdate` | `Фишка не найдена`; `Игрок не найден` |
| `setTokenImage` | `{ tokenId, image }`, async | joined (dm или владелец) | `imageUrl`, сброс кадрирования, `avatarFromSheet=false` | `tokenUpdated` | `Фишка не найдена`; `Можно менять изображение только своей фишки`; `Не удалось сохранить изображение фишки` |
| `setTokenGroupImage` | `{ name, image }`, async | dm | `imageUrl` всем фишкам с тем же `groupKey(name)` | `D→tokensUpdated {tokens: полные}`, `io.except('dm-sockets')→tokensUpdated {tokens: видимые через tokenForViewer}` | `Не указано название группы`; `Фишек с таким названием нет`; `Не получилось сохранить картинку` |
| `setTokenLight` | `{ tokenId, radius, color? }` | dm | `token.light` | `tokenUpdated` | `Фишка не найдена`; `Дальность света задана неверно` |
| `setTokenDefenses` | `{ tokenId, resistances?, vulnerabilities?, immunities? }` (массив или строка с запятыми) | dm | `damageResistances`, `damageVulnerabilities`, `damageImmunities` (только известные типы, до 16) | `tokenUpdated` | `Фишка не найдена` |
| `setTokenConditions` | `{ tokenId, conditions[], conditionDurations{}, conditionDurationUnits{} }` | dm | состояния и счётчики | `tokenUpdated` | `Фишка не найдена` |
| `setTokenAura` | `{ tokenId, feet, color? }` (`feet <= 0` → снять) | dm | `aura` (5..120 футов) | `tokenUpdated` | `Фишка не найдена` |
| `setTokenVision` | `{ tokenId, vision }` (0..240) | dm | `vision` | `tokenUpdated` | `Фишка не найдена` |
| `moveToken` | `{ tokenId, x, y, final?, from?:{col,row}, path?:[{col,row}] }` | joined (dm или владелец) | `x`,`y`, `movedCells` при `final`+`path`, автотуман при `fogFollow` | `tokenMoved` (+`final`, `from`, `path`, `spent`, `movedCells`), при автотумане `mapUpdated` | `Фишка не найдена`; `Можно двигать только свою фишку`; `Фишка «<состояние>»: двигать её нельзя, пока состояние не снято`; `Истощение 5: скорость 0, двигать фишку нельзя` |
| `moveTokenToActiveMap` | `{ tokenId }` | dm | `mapType/mapId` = активная показанная, `x=y=0.5` | `tokenCreated` | `Фишка не найдена`; `Сначала покажите карту игрокам`; `Эта карта ещё не показана игрокам`; `Фишка уже на этой карте` |
| `moveAllTokensToActiveMap` | — | dm | перенос всех чужих карт сеткой 3 в ряд | `tokenCreated` по каждой, `playersUpdate` | `Сначала покажите карту игрокам`; `Эта карта ещё не показана игрокам`; `Все фишки уже на этой карте` |
| `deleteToken` | `{ tokenId }` | dm | удаление, `dropInitiativeByRef` | `tokenDeleted {tokenId}` (D + P если была видима), `initiativeUpdated`, `playersUpdate` | `Фишка не найдена` |
| `setDmNote` | `{ playerId, note }` | dm | `players[x].dmNote` | `D→dmNoteUpdated {playerId,note}` | `Игрок не найден` |

### 2.5 Карты, туман, стены, свет, шаблоны

`findMapItem(payload)`: вкладка `payload.type` (`world`\|`battle`\|`sketch`), карта `payload.mapId`; без `mapId` — активная (`slot.activeId`) или первая. Возвращает `{type, item}`; пустая вкладка → `{type:'world', item:null}`.

| Событие | payload | Кто | Мутации | Исходящие | Ошибки |
|---|---|---|---|---|---|
| `uploadMap` | `{ type, image, name? }`, async | dm | push карты `{id,url,name,published:false,fogMask:newFogMask(),audioRef:null,surface:{cells:{}},walls:[],lights:[],darkness:0,uploadedAt}`; `slot.activeId = id`; `pushUndo` | `mapUpdated` (D и P) | `Неизвестный тип карты`; `В этой вкладке уже 20 карт — удалите лишние`; `Не удалось сохранить изображение карты`; ошибки `decodeImage` |
| `selectMap` | `{ type, mapId }` | dm | `slot.activeId` (без показа игрокам) | `mapUpdated` | `Неизвестный тип карты`; `Карта не найдена` |
| `deleteMap` | `{ type, mapId? }` | dm | удаление карты, сцен с ней, пересчёт `activeMap`/`activeMapId`, `pushUndo` | `mapUpdated` (своя вкладка и, при смене, чужая), `scenesUpdated` | `Неизвестный тип карты`; `Карта не найдена` |
| `setActiveMap` | `{ type, mapId }` | dm | `published = true`, `activeMap`, `activeMapId`, `pushUndo` | `mapUpdated`, при первом показе `tokenCreated` по фишкам карты | `Неизвестный тип карты`; `Карта не найдена` |
| `openMapFog` | `{ type, mapId?, x, y, radius }` | dm | `fogMask`; `pushUndo` | `mapUpdated` | `Карта не найдена` |
| `setMapFog` | `{ type, mapId?, open }` | dm | туман целиком; `pushUndo` | `mapUpdated` | `Карта не найдена` |
| `setMapFogFollow` | `{ type, mapId?, follow }` | dm | `fogFollow` | `mapUpdated` | `Карта не найдена` |
| `setMapVision` | `{ type, mapId?, visionPerPlayer }` | dm | `visionPerPlayer` | `mapUpdated` | `Карта не найдена` |
| `paintMapSurface` | `{ type, mapId?, cells: [[col,row,kind\|null], ...] }`, до 400 | dm | `surface.cells`; `pushUndo` | `mapUpdated` | `Карта не найдена` |
| `clearMapSurface` | `{ type, mapId? }` | dm | `surface = {cells:{}}`; `pushUndo` | `mapUpdated` | `Карта не найдена` |
| `setMapDarkness` | `{ type, mapId?, darkness 0..1 }` | dm | `darkness`, `autoLight = false` | `mapUpdated` | `Карта не найдена` |
| `setMapAutoLight` | `{ type, mapId?, on }` | dm | `autoLight`, при включении `darkness = darknessForHour(hour)` | `mapUpdated` | `Карта не найдена` |
| `setMapPin` | `{ type, mapId?, pin:{ id?, x, y, title, text }, pinId? }` | dm | `pins` (до 60, старейшая вытесняется) | `mapUpdated` (пины игрокам вырезаются) | `Карта не найдена` |
| `deleteMapPin` | `{ type, mapId?, pinId }` | dm | `pins` | `mapUpdated` | `Карта не найдена`; `Булавка не найдена` |
| `setMapWallDoor` | `{ type, mapId?, wallId, door?:{open,locked}\|null, window?:bool\|null }` | dm | `wall.door`, `wall.window`, `normalizeWalls` | `mapUpdated` | `Карта не найдена`; `Стена не найдена` |
| `addMapWall` | `{ type, mapId?, wallId?, x1,y1,x2,y2 }` | dm | push стены (id от клиента, иначе `newTokenId()`) | `mapUpdated` | `Карта не найдена`; `Стен больше 400 быть не может` |
| `deleteMapWall` | `{ type, mapId?, wallId }` | dm | удаление стены | `mapUpdated` | `Карта не найдена` |
| `clearMapWalls` | `{ type, mapId? }` | dm | `walls = []` | `mapUpdated` | `Карта не найдена` |
| `addMapTemplate` | `{ type, mapId?, templateId?, kind, x, y, size, rotation, color }` | dm | push шаблона; `pushUndo` | `mapUpdated` | `Карта не найдена`; `Шаблонов больше 20 быть не может` |
| `deleteMapTemplate` | `{ type, mapId?, templateId }` | dm | удаление; `pushUndo` | `mapUpdated` | `Карта не найдена` |
| `clearMapTemplates` | `{ type, mapId? }` | dm | `templates = []`; `pushUndo` | `mapUpdated` | `Карта не найдена` |
| `addMapLight` | `{ type, mapId?, lightId?, x, y, radius, color, dim }` | dm | push источника (id от клиента) | `mapUpdated` | `Карта не найдена`; `Источников света больше 80 быть не может` |
| `setMapLight` | `{ type, mapId?, lightId, x?, y?, radius?, color?, dim?, hidden? }` | dm | замена полей источника | `mapUpdated` | `Карта не найдена`; `Источник света не найден`; `Источник света задан неверно` |
| `deleteMapLight` | `{ type, mapId?, lightId }` | dm | удаление | `mapUpdated` | `Карта не найдена` |
| `mapPing` | `{ mapType, mapId, x, y }` | joined | нет; не чаще 250 мс на сокет; только активная показанная карта | `T→mapPing {mapType,mapId,x,y,playerId,name,role}` | молча |
| `setGrid` | `{ enabled, cell, columns, feetPerCell, offsetX, offsetY }` | dm | `normalizeGrid({...room.grid, ...payload, enabled: Boolean(payload.enabled)})`, выход если не изменилось | `IO→gridUpdated` | — |
| `setMapAudio` | `{ type, mapId?, ref }` | dm | `audioRef` (только известный звук) | `mapUpdated` | `Карта не найдена`; `Звук не найден на саундборде` |

### 2.6 Время, календарь, таймер, отдых

| Событие | payload | Кто | Мутации | Исходящие | Ошибки |
|---|---|---|---|---|---|
| `setGameTime` | `{ day, hour }` | dm | `gameTime`, тик состояний в часах, авто-свет | `mapUpdated` по вкладкам, `IO→gameTimeUpdated`, `tokenUpdated` по истёкшим | молча, если время не изменилось |
| `setCalendarMonths` | `{ months: string[] }`, ack | dm | `calendarMonths` (до 24, пусто → дефолтные) | `IO→calendarMonthsUpdated` | ack `{ok:false,error:'Это действие доступно только ДМ'}` |
| `addCalendarEvent` | `{ title, day }`, ack | dm | событие (до 200) | `IO→calendarEventsUpdated` | `Слишком много событий в календаре`; `Нужно название и день события` (в `errorMessage` и ack) |
| `deleteCalendarEvent` | `{ id }`, ack | dm | удаление | `IO→calendarEventsUpdated` | `Событие не найдено` |
| `setPartyNotes` | `{ text }`, ack | joined | `partyNotes` (20000) | `IO→partyNotesUpdated` при изменении | — |
| `timerStart` | `{ seconds, label? }` | dm | `timer = {running:true, endsAt: now+sec*1000, totalSec, label}`, до 4 ч | `IO→timerUpdated` | `Укажите длительность таймера` |
| `timerStop` | — | dm | `timer = emptyTimer()` | `IO→timerUpdated` | молча, если не шёл |
| `partyLongRest` | — | dm | все листы: hp=max, tempHp=0, спасброски 0, ячейки 0, кости хитов +половина, концентрация '', истощение −1; `gameTime += 8 ч`; тик состояний; авто-свет | `characterSheetData`+`characterSheetUpdated`+`tokenUpdated` по каждому, `IO→gameTimeUpdated`, `tokenUpdated` по истёкшим, chat-уведомление | — |
| `awardXp` | `{ amount 0..999999, targets?: string[] }` | dm | `sheet.experience` каждому получателю | `characterSheetData`/`characterSheetUpdated` по каждому, chat-подсказка при переходе порога, chat-объявление | `Укажите сумму опыта`; `Некому выдавать опыт: нет листов игроков`; `Сумма мельче, чем один опыт на игроков` |

### 2.7 Кинорежим

| Событие | payload | Кто | Мутации | Исходящие | Ошибки |
|---|---|---|---|---|---|
| `setShowcase` | `{ on, kind?:'image'\|'text', ref?, text?, title? }` | dm | `showcase {on, kind, ref, text, title, watchers:[]}`; `ref` только `/^\/files\//`, до 120 символов | `IO→showcaseUpdated` | `К кинорежиму нужна картинка из файлов стола` |
| `showcaseSeen` | — | joined | `watchers.push(playerId)` | `IO→showcaseUpdated` | молча, если показа нет |

### 2.8 Звук

| Событие | payload | Кто | Мутации | Исходящие | Ошибки |
|---|---|---|---|---|---|
| `uploadSound` | `{ sound, name? }`, async | dm | push в `soundboard` `{id,name,ref}`, до 40 | `D→soundboardUpdated` | `На саундборде уже 40 звуков — удалите лишние`; `Не удалось сохранить звук`; ошибки `decodeAudio` |
| `deleteSound` | `{ id }` | dm | удаление; у карт с этим `audioRef` → `null` | `mapUpdated` по затронутым вкладкам, `D→soundboardUpdated` | молча |
| `playSound` | `{ ref, action?:'play'\|'loop'\|'stop' }` | joined | нет | `socket.broadcast.emit→soundCue {ref, action}` (всем, кроме отправителя) | молча, если звук неизвестен |

### 2.9 Инициатива и режим боя

| Событие | payload | Кто | Мутации | Исходящие | Ошибки |
|---|---|---|---|---|---|
| `initiativeAdd` | `{ kind:'player'\|'token'\|'other', refId?, name?, modifier? }` | dm | push записи `{id,kind,refId,name,modifier,total:null}`, до 40 | `IO→initiativeUpdated` | `Такой участник боя уже есть в списке` (в т.ч. при достижении лимита) |
| `initiativeRemove` | `{ id }` | dm | удаление; перенос хода; сброс движения | `initiativeUpdated` | `Участник боя не найден` |
| `initiativeRoll` | `{ id?, modifier? }` | dm | d20(+помеха при истощении) + модификатор, сортировка; без `id` — новый раунд, `turnId = первый`, сброс движения и реакций, `combat.enabled = true` | `initiativeUpdated`, при старте боя `IO→combatMode` | `Список боя пуст`; `Участник боя не найден` |
| `initiativeSetTotal` | `{ id, total }` | dm | `entry.total` | `initiativeUpdated` | `Участник боя не найден`; `Итог должен быть числом` |
| `initiativeTurn` | `{ id }` | dm | `turnId`, сброс движения, тик состояний текущего хода | `initiativeUpdated`, `tokenUpdated` | `Участник боя не найден` |
| `initiativeStep` | `{ dir }` | dm | следующий/предыдущий, новый раунд с последнего, тик состояний | `initiativeUpdated`, `tokenUpdated` | `Список боя пуст` |
| `initiativeReset` | — | dm | `resetInitiative`, выключает режим боя | `initiativeUpdated`, `combatMode` | — |
| `setCombatMode` | `{ enabled }` | dm | `combat.enabled` | `IO→combatMode`, chat-уведомление; при том же значении `S→combatMode` | — |

Порядок инициативы: `total` desc, при равенстве `modifier` desc, затем порядок добавления. `step` с последнего на первый: `round += 1`, реакции прошлых раундов удаляются.

### 2.10 Кубики, проверки, реакции

| Событие | payload | Кто | Мутации | Исходящие | Ошибки |
|---|---|---|---|---|---|
| `rollDice` | `{ formula? \| sides?, hidden?, dmOnly?, label?, targetAc? }` | joined | `diceHistory` (кроме `dmOnly`); при провале заявки «Концентрация: …» — снятие концентрации | `P→diceRollResult` (без чисел для hidden), `D→diceRollResult`, `dmOnly`: `D→diceRollResult` + `D→dmRollResult`; снятие концентрации: `characterSheetData/Updated`, `tokenUpdated`, `IO→chatMessage`, `rollRequestUpdated` | ошибки `parseFormula` |
| `rollTable` | `{ tableId, hidden? }` | dm | нет | обычный: `IO→diceRollResult`; `hidden`: `D→dmRollResult` | `Таблица с такими строками не найдена` |
| `rollRequest` | `{ label, dc?, formula?, to? }`, ack | dm | push заявки (до 20) | `D→rollRequestUpdated` (все), каждому игроку своя выборка | `Игрок для проверки не найден`; `Нужно название проверки` |
| `rollRequestClear` | `{ id }`, ack | dm | удаление заявки | обе выборки `rollRequestUpdated` | — |
| `reactionPrompt` | `{ playerId, attackerName, targetName, bonus?, targetAc? }` | dm | push реакции (до 5, вытесняется старейшая) | `R(playerId)→reactionPrompt` | `Игрок не найден` |
| `reactionResolved` | `{ id }` | joined (dm или адресат) | удаление реакции | `R(removed.playerId)→reactionResolved {id}` | молча |

`rollDice`: `hidden` действует только у ДМ; `dmOnly` — только у ДМ и не пишется в историю; `targetAc` (1..60) даёт `hit` и `note`, только если `count === 1 && sides === 20` и бросок не `hidden`.

### 2.11 Чат

| Событие | payload | Кто | Мутации | Исходящие | Ошибки |
|---|---|---|---|---|---|
| `chatMessage` | `{ text, whisper?, to? }` | joined | push в `chatHistory` (300) | шёпот: `D`, `R(to)`, `R(свой id)` если автор игрок; иначе `IO` | текст пустой → молча |
| `deleteChatMessage` | `{ id }` | dm | удаление из истории | `IO→chatDeleted {id}` | молча |
| `editChatMessage` | `{ id, text }`, ack | dm | `text`, `editedAt` | шёпот: `D` + `R(to)` + `R(автора)`; иначе `IO→chatEdited {id,text,editedAt}` | `Сообщение не найдено`; `Системные сообщения не редактируются`; `Текст сообщения пуст` |
| `clearChat` | — | dm | `chatHistory = []` | `IO→chatCleared` | молча, если пусто |
| `chatImage` | `{ image, text?, to? }`, async | dm | файл + сообщение в `chatHistory` | шёпот: `D` + `R(to)`; иначе `IO` | `Игрок для изображения не найден`; `Не получилось сохранить изображение`; ошибки `decodeImage` |

Шёпот: игрок всегда шлёт мастеру (`to = 'dm'`), адресата выбрать не может. Мастер шлёт выбранному игроку. Несуществующий `to` у мастера **молча превращает сообщение в общее** (см. раздел 7).

### 2.12 Журнал, склад, опросы, сцены

| Событие | payload | Кто | Мутации | Исходящие | Ошибки |
|---|---|---|---|---|---|
| `saveJournal` | `{ text }`, ack | dm | `journal` (20000); `pushUndo` | нет события (только ack) | ack `{ok:false,error:'Это действие доступно только ДМ'}` |
| `sessionLogExport` | —, ack | dm | нет | ack `{ok, text, name}` | `Экспорт доступен только мастеру`; `Хронология сессии пуста` |
| `saveJournalEntry` | `{ id?, kind?, title, text?, image?, tags?, visible? }`, ack | dm | upsert записи (300) | `D→journalUpdated` (все), `P→journalUpdated` (visible) | `Нужен заголовок записи` |
| `journalImage` | `{ image, title? }`, ack | dm | файл + запись `kind:'item'` | `journalUpdated` (D и P) | `Не удалось сохранить снимок`; ошибки `decodeImage` |
| `deleteJournalEntry` | `{ id }`, ack | dm | удаление | `journalUpdated` | `Запись не найдена` |
| `journalRequest` | — | joined | нет | `S→journalUpdated` (ДМ — все, игрок — visible) | — |
| `saveInventoryItem` | `{ id?, name, qty?, weight?, ownerId?, attunement?, notes? }`, ack | dm | upsert (500) | `T→inventoryUpdated` | `Нужно название предмета` |
| `deleteInventoryItem` | `{ id }`, ack | dm | удаление | `T→inventoryUpdated` | `Предмет не найден` |
| `setInventoryCoins` | `{ coins }`, ack | dm | `inventoryCoins` | `T→inventoryCoinsUpdated` | — |
| `splitInventoryCoins` | —, ack | dm | делит поровну между игроками с листами, остаток в складе, листы без undo | `IO→inventoryCoinsUpdated`, `characterSheetData`/`characterSheetUpdated` по игрокам | `Нечего делить: нет игроков с листами` |
| `giveInventoryCoins` | `{ playerId, coin, amount }`, ack | dm | перевод со склада в лист | `IO→inventoryCoinsUpdated`, `characterSheetData`/`characterSheetUpdated` | `Неизвестная монета`; `Игрок не найден`; `У игрока нет листа`; `Укажите количество`; `На складе нет этих монет`; `В кошельке листа нет места` (текст приходит из `state.js`) |
| `startVote` | `{ question, options[] }` | dm | `vote = {question, options (2..6), votes:{}}` | `IO→chatMessage`, `D→voteUpdated` (с голосами), каждому игроку `voteUpdated` (с `counts`, `yourVote`) | `Нужен вопрос опроса`; `Нужно минимум два варианта` |
| `voteCast` | `{ optionIndex }` | joined | `vote.votes[playerId] = index` | как выше | `Опрос не идёт`; `Нет такого варианта` |
| `voteClosed` | — | dm | `vote = null` | `IO→voteUpdated null` | — |
| `saveScene` | `{ sceneId?, name, mapType, mapId }`, ack | dm | снимок карты/тумана/света/сетки/фишек (20) | `D→scenesUpdated` | `Только ДМ`; `Карта не найдена`; `Сцена не найдена`; `Больше 20 сцен не сохраняется` |
| `loadScene` | `{ sceneId, show? }`, ack | dm | `applyScene` (атмосфера, сетка, фишки; при `show` — публикация карты и перенос героев); `pushUndo` внутри | каждому сокету `stateReload`, `D→sceneLoaded {sceneId,mapType,mapId,show}` | `Только ДМ`; `Сцена не найдена`; `Карта сцены не найдена` |
| `renameScene` | `{ sceneId, name }`, ack | dm | `name`, `updatedAt` | `D→scenesUpdated` | `Только ДМ`; `Сцена не найдена`; `Пустое имя сцены` |
| `duplicateScene` | `{ sceneId }`, ack | dm | копия в начало списка, имя `«<имя> (копия)»` | `D→scenesUpdated` | `Только ДМ`; `Сцена не найдена`; `Больше 20 сцен не сохраняется` |
| `deleteScene` | `{ sceneId }`, ack | dm | удаление | `D→scenesUpdated` | `Только ДМ`; `Сцена не найдена` |
| `undoDmAction` | —, ack | dm | `undoLast()`: maps, tokens, initiative, journal, journalEntries, inventory, inventoryCoins, gameTime, calendarMonths, calendarEvents, activeMap, activeMapId | каждому сокету `stateReload` | `Нечего отменять` |

### 2.13 Приключение и бестиарий

| Событие | payload | Кто | Мутации | Исходящие | Ошибки |
|---|---|---|---|---|---|
| `importSrd` | —, ack | dm | добавляет новых actors/spells из `data/srd.json` с дедупом по имени, до лимитов | `D→adventureUpdated` | `Импорт доступен только мастеру`; `В компендиуме нет ничего нового` |
| `setAdventureGroupImage` | `{ section:'items'\|'actors', name, image }`, async | dm | `image` всей группе по `groupKey(name)` | `D→adventureUpdated` | `В кампании нет приключения`; `Картинку массово можно назначить только предметам и персонажам`; `Не указано название группы`; `В приключении нет объектов с таким названием`; `Не получилось сохранить картинку` |
| `deleteAdventureActor` | `{ name }` | dm | удаляет всю группу по имени | `D→adventureUpdated` | `В кампании нет приключения`; `Не указано название персонажа`; `В приключении нет такого персонажа` |
| `deleteAdventureSpell` | `{ name }` | dm | удаляет группу заклинаний | `D→adventureUpdated` | `В кампании нет приключения`; `Не указано название заклинания`; `В приключении нет такого заклинания` |
| `setAdventureActorFavorite` | `{ name, favorite }` | dm | `favorite` всей группе | `D→adventureUpdated` | `В кампании нет приключения`; `Не указано название персонажа`; `В приключении нет такого персонажа` |
| `setAdventureGroupStatblock` | `{ name, statblock, create? }` | dm | `statblock` группе; при `create` создаёт запись | `D→adventureUpdated` | `Не указано название персонажа`; `В приключении нет персонажа с таким названием`; `В приключении уже слишком много персонажей` |
| `createTokenFromAdventureActor` | `{ name, mapType?, mapId?, count? 1..20, x?, y? }`, ack | dm | до `count` скрытых фишек сеткой 3×N | `tokenCreated` по каждой | `Это действие доступно только ДМ`; `Не указано название персонажа`; `В кампании нет приключения`; `Слишком много фишек на столе`; `В приключении нет такого персонажа`; `Лимит фишек на столе уже достигнут` |

---

## 3. Исходящие события

| Событие | payload | Кому |
|---|---|---|
| `joined` | `{ you:{playerId,name,role,playerKey}, state: snapshotFor }` | `S` |
| `errorMessage` | `{ message }` | `S` |
| `pong` | `{ time, echo }` | `S` |
| `stateReload` | полный `snapshotFor` | каждому сокету с ролью (включая ДМ) |
| `playersUpdate` | `playerList[]`; игрокам без `dmNote` и без скрытых фишек | `P` и `D` отдельными вызовами |
| `mapUpdated` | `{ type, items[], activeId, activeMap, activeMapId }`; игрокам только `published`, без `pins` | `D` (полный) и `P` |
| `gridUpdated` | `{ enabled, cell, columns, feetPerCell, offsetX, offsetY }` | `IO` |
| `gameTimeUpdated` | `{ day, hour }` | `IO` |
| `calendarMonthsUpdated` | `string[]` | `IO` |
| `calendarEventsUpdated` | `[{id,day,title}]` | `IO` |
| `partyNotesUpdated` | `string` | `IO` |
| `timerUpdated` | `{ running, endsAt, totalSec, label }` | `IO` |
| `showcaseUpdated` | `{ on, kind, ref, text, title, watchers[] }` | `IO` |
| `combatMode` | `{ enabled }` | `IO` (или `S` при повторном значении) |
| `initiativeUpdated` | `{ entries:[{id,kind,refId,name,modifier,total,roll?}], turnId, round }` | `IO` |
| `tokenCreated` | токен (ДМ — полный, игрок — `tokenForViewer(false)`) | `D` (+`P`, если видима) |
| `tokenUpdated` | то же | `D` (+`P`, если видима) |
| `tokenMoved` | `{ tokenId, x, y, final?, from?, path?, spent?, movedCells? }` — **явный payload**, фильтрация не применяется | `D` (+`P`, если видима) |
| `tokenDeleted` | `{ tokenId }` | `D` (+`P`, если видима) |
| `tokenHidden` | `{ tokenId, hidden }` | скрытие — `IO`; показ — `D` |
| `tokensUpdated` | `{ tokens: [] }` | `D` (полные) и все остальные (видимые, обрезанные) |
| `characterSheetData` | `{ playerId, sheet }` | `R(playerId)` |
| `characterSheetUpdated` | `{ playerId, sheet }` | `D` |
| `dmNoteUpdated` | `{ playerId, note }` | `D` |
| `diceRollResult` | результат `dice.rollFormula` + `playerId, playerName, role, hidden, time, label?, targetAc?, hit?, note?`; игрокам hidden приходит без `result/values/keptValues/droppedValues` | `IO` / `P`+`D` раздельно / `D` |
| `dmRollResult` | как `diceRollResult` + `hidden:true`, `label` | `D` |
| `chatMessage` | `{ id, from, playerId, role, text, image?, whisper?, to?, toName?, system?, isRoll?, time, editedAt? }` | `IO` / `D`+`R(to)`(+`R(автор)`) |
| `chatDeleted` | `{ id }` | `IO` |
| `chatEdited` | `{ id, text, editedAt }` | `IO` / шёпот: `D`+`R(to)`+`R(автор)` |
| `chatCleared` | — | `IO` |
| `soundCue` | `{ ref, action }` | все, кроме отправителя |
| `soundboardUpdated` | `[{id,name,ref}]` | `D` |
| `mapPing` | `{ mapType, mapId, x, y, playerId, name, role }` | `T` |
| `reactionPrompt` | `{ id, playerId, attackerName, targetName, bonus, targetAc, round }` | `R(playerId)` |
| `reactionResolved` | `{ id }` | `R(removed.playerId)` |
| `rollRequestUpdated` | `[{ id, label, dc, formula, to?, toName?, createdAt, by }]` | `D` (все) и каждому игроку (без чужих `to`) |
| `journalUpdated` | записи журнала | `D` (все) / `P` и `S` (только `visible`) |
| `inventoryUpdated` | `[{id,name,qty,weight,ownerId,attunement,notes}]` | `T` |
| `inventoryCoinsUpdated` | `{ cp, sp, ep, gp, pp }` (ключи монет) | `T` или `IO` |
| `voteUpdated` | `null`; ДМ: `{question, options, votes}`; игрок: `{question, options, counts, yourVote}` | `IO` / `D` / `R(playerId)` |
| `scenesUpdated` | `[{id,name,mapType,mapId,mapName,mapUrl,tokenCount,published,createdAt,updatedAt}]` | `D` |
| `sceneLoaded` | `{ sceneId, mapType, mapId, show }` | `D` |
| `adventureUpdated` | весь `adventure` | `D` |

---

## 4. Домены и кто чем владеет

| Домен | События | Писатель | Куда уходит результат |
|---|---|---|---|
| Чат | `chatMessage`, `chatImage`, `editChatMessage`, `deleteChatMessage`, `clearChat` | все / ДМ на правку | `IO`, шёпот — `D`+адресат+автор |
| Токены | `createToken`, `createTokenFromSheet`, `createTokenFromAdventureActor`, `renameToken`, `deleteToken`, `moveToken`, `moveTokenToActiveMap`, `moveAllTokensToActiveMap`, `assignTokenToPlayer`, `setToken*`, `tokensBatch`, `setAllTokensHidden` | ДМ, движение и картинка/эмоут — владелец | `emitToken` (D + P с фильтрацией), `playersUpdate` |
| Карты | `uploadMap`, `selectMap`, `deleteMap`, `setActiveMap`, `setMapAudio` | ДМ | `mapUpdated` (D полный, P только published) |
| Туман | `openMapFog`, `setMapFog`, `setMapFogFollow` | ДМ | `mapUpdated` |
| Стены | `addMapWall`, `deleteMapWall`, `clearMapWalls`, `setMapWallDoor` | ДМ | `mapUpdated` |
| Свет | `setMapDarkness`, `setMapAutoLight`, `addMapLight`, `setMapLight`, `deleteMapLight`, `setTokenLight`, `setTokenVision`, `setMapVision` | ДМ | `mapUpdated` / `tokenUpdated` |
| Шаблоны | `addMapTemplate`, `deleteMapTemplate`, `clearMapTemplates`, `castAoe` | ДМ | `mapUpdated`, броски |
| Поверхность | `paintMapSurface`, `clearMapSurface` | ДМ | `mapUpdated` |
| Инициатива | `initiative*`, `setCombatMode` | ДМ | `initiativeUpdated`, `combatMode` |
| Листы | `saveCharacterSheet`, `castSpell`, `rollDeathSave`, `resetDeathSaves`, `requestCharacterSheet` | игрок по своему листу | `characterSheetData` владельцу, `characterSheetUpdated` ДМ |
| Журнал | `saveJournal`, `saveJournalEntry`, `journalImage`, `deleteJournalEntry`, `journalRequest`, `sessionLogExport` | ДМ | `journalUpdated` (фильтр по `visible`) |
| Звук | `uploadSound`, `deleteSound`, `setMapAudio`, `playSound` | ДМ; `playSound` — любой joined | `soundboardUpdated`, `soundCue` |
| Опросы и таймер | `startVote`, `voteCast`, `voteClosed`, `timerStart`, `timerStop` | ДМ, голос — все | `voteUpdated`, `timerUpdated` |
| Приключение | `importSrd`, `setAdventure*`, `deleteAdventure*`, `createTokenFromAdventureActor` | ДМ | `adventureUpdated` (только ДМ) |
| Сцены | `saveScene`, `loadScene`, `renameScene`, `duplicateScene`, `deleteScene` | ДМ | `scenesUpdated`, `stateReload` |
| Календарь | `setCalendarMonths`, `addCalendarEvent`, `deleteCalendarEvent`, `setGameTime`, `partyLongRest` | ДМ | `calendar*Updated`, `gameTimeUpdated` |
| Служебные | `joinRoom`, `ping`, `disconnect`, `undoDmAction` | все | `joined`, `pong`, `playersUpdate`, `stateReload`, `errorMessage` |

---

## 5. Неочевидные инварианты

### 5.1 Лимиты (`state.js`, `LIMITS`)

`maxNameLength 40`, `maxChatLength 1000`, `maxSheetTextLength 4000`, `maxImageBytes 10 МБ`, `maxAvatarBytes 2 МБ`, `maxAudioBytes 2 МБ`, `maxDiceHistory 200`, `maxInitiativeEntries 40`, `maxChatHistory 300`, `maxTokens 200`, `maxTokenHp 9999`, `maxTokenEmoteLength 80`, `maxDeathSaves 3`, `maxTokenConditions 16`, `maxExhaustion 6`, `maxInspiration 99`, `maxConcentrationLength 120`, `maxCoins 999999`, `maxSpellSlotsPerLevel 9`, `maxDmNote 4000`, `maxJournal 20000`, `maxJournalEntries 300`, `maxJournalTitle 120`, `maxJournalText 8000`, `maxJournalTags 8`, `maxJournalTag 32`, `maxCalendarEvents 200`, `maxCalendarTitle 120`, `maxPartyNotes 20000`, `maxInventoryItems 500`, `maxInventoryName 120`, `maxInventoryNotes 1000`, `maxRollRequests 20`, `maxSheetAttacks 20`, `maxLevel 20`, `minAbility 1`, `maxAbility 30`, `maxMapsPerType 20`, `maxScenes 20`, `maxSceneTokens 200`, `maxSceneName 60`, `maxAdventureNotes 300`, `maxAdventureItems 500`, `maxAdventureTables 200`, `maxAdventureTableRows 100`, `maxSheetSpells 60`, `maxAdventureActors 200`, `maxAdventureSpells 400`, `maxMapPins 60`, `maxAdventureTitle 120`, `maxAdventureText 20000`, `maxAdventureImageRef 2048`, `maxAvatarRef 2048`, `maxSoundboard 40`, `maxAudioRef 2048`.

Из `renderer/dnd5e.js`: `LIGHT_LIMITS { maxWalls: 400, maxLights: 80, maxPins: 60, minRadius: 1, maxRadius: 200, rays: 64 }`; `TEMPLATE_LIMITS { maxPerMap: 20, minSize: 5, maxSize: 120 }`; `GRID { minCell: 8, maxCell: 512, defaultCell: 46, minColumns 2, maxColumns 200, minColumnsStep 0.5, maxOffset 4096, minFeetPerCell 1, maxFeetPerCell 50, defaultFeetPerCell 5 }`; суточная кривая `DARKNESS_BY_HOUR[24]`, `darknessForHour(hour) = DARKNESS_BY_HOUR[round(hour) clamped 0..23]`.

### 5.2 Санитизация

- `sanitizeText(value, max)`: `null`/`undefined` → `''`, иначе `String`, удаление `\u0000`, обрезка до `max` **по UTF-16 code units** (может разрезать суррогатную пару и эмодзи). HTML не экранируется: экранирование — забота клиента.
- `sanitizeName`: `sanitizeText(raw, 40).replace(/\s+/g,' ').trim()`.
- `groupKey(name)`: `String(name).replace(/\s+/g,' ').trim().toLowerCase()` — ключ групповых операций по имени.
- `normalizeDamageDefenses`: массив или строка с запятыми → только `DAMAGE_TYPE_IDS`, до 16.
- `clampInt/clampNumber(value, min, max, fallback)`: нечисло → `fallback`, иначе зажим.
- Загрузки: `decodeImage` проверяет сигнатуру (PNG/JPG/WEBP), лимит байт; тексты ошибок общие для всех картинок: `Изображение не передано`, `Поддерживаются только изображения PNG, JPG и WEBP`, `Файл пустой`, `Файл больше <N> МБ`, `Формат <label> не поддерживается. Сохраните изображение как PNG, JPG или WEBP.`, `Файл не является изображением PNG, JPG или WEBP.`. Звук — `decodeAudio` со своими текстами.
- В события уходит только `/files/<имя>`; base64 в состоянии не хранится (есть разовая миграция `migrateLegacyImages` при старте).

### 5.3 Скрытые броски

- `hidden` принимается только от ДМ; у игрока игнорируется.
- Игрокам уходит `hideRollResult(roll)` = без `result`, `values`, `keptValues`, `droppedValues`, `result: null`.
- `dmOnly` — броски статблока: только `D`, в `diceHistory` не пишется, в чат не идёт.
- `snapshotFor` для игрока прогоняет `diceHistory` через `hideRollResult`.

### 5.4 Приватные сообщения

- Шёпот хранится в `chatHistory` с `whisper: true`, `to`, `toName`.
- `snapshotFor` игрока фильтрует: `!whisper || playerId === свой || to === свой`.
- Доставка: `D` + `R(to)` + `R(автора)`. Игрок в комнате `dm` не состоит, поэтому копия автору нужна отдельно.
- Скрытая заявка (`rollRequest.to`) уходит только ДМ и адресату; в снимке фильтр тот же.
- Скрытая реакция: `snapshotFor` игрока отдаёт только свои.

### 5.5 Владелец токена

Проверка «своя фишка» = `token.assignedPlayerId === socket.data.playerId`. Применяется в `moveToken`, `setTokenImage`, `setTokenEmote`. Всё остальное — только ДМ.

`moveToken` дополнительно для игрока: `moveBlocker(token)` (состояние, запрещающее движение) и истощение 5 (`speedMultiplier === 0`).

### 5.6 Здоровье: один источник

- У фишки с владельцем здоровье живёт в листе. `setTokenHp`, `applyTokenDamage`, `syncSheetTokens`, `tokenForViewer` читают/пишут лист.
- Временные хиты: сначала съедаются они (`sheet.tempHp`), остаток идёт в `hp`.
- Защиты фишки (`damageResistances/Vulnerabilities/Immunities`) применяются до вычитания; тип урона нормализуется в нижний регистр.
- Смерть: урон по лежащему (hp ≤ 0) — +1 провал; `hpDamage >= max` — мгновенная смерть (3 провала); массивный урон (`remaining >= max`) — тоже.
- Концентрация: СЛ `max(10, floor(amount / 2))`. Игроку создаётся/поднимается заявка «Концентрация: <спелл> (СЛ N)», у монстра сервер бросает сам и на провале стирает `token.concentration`.

### 5.7 Уведомления об уроне

`announceTokenHp` только для фишек с `assignedPlayerId` и видимых игрокам. Склейка 500 мс: повторный урон в окне сбрасывает таймер, но `before`, `type`, `defense` и автор остаются от первого удара. У монстров объявлений нет. Тексты: `<имя>: <hp>/<maxHp> (урон N, <тип>)`, `<имя>: <hp>/<maxHp> (лечение N)`, при иммунитете `<имя>: иммунитет к урону (<тип>)`.

### 5.8 Порядок рассылки и `markDirty`

- Во всех мутирующих обработчиках мутация предшествует рассылке; `markDirty()` может стоять до или после emit — на клиента это не влияет, на автосейв влияет только факт вызова.
- `broadcastPlayers` — две отдельные рассылки: игрокам (`includeDmNotes:false, includeHidden:false`) и ДМ (`true/true`). Скрытые заметки и заготовки не утекают.
- `broadcastMapUpdate` — две рассылки; игрокам вырезаются неопубликованные карты и `pins`.
- `emitToken` — ДМ всегда, игроки только если фишка видима, и только `tokenForViewer(token,false)` либо явный payload.
- `broadcastSnapshot` обходит `io.sockets.sockets` и шлёт `stateReload` каждому, у кого есть роль.
- `diagnostics`: `onLog` — только строка в лог, состояние не трогает.

### 5.9 Undo

`pushUndo` кладёт JSON-снимок: `maps, tokens, initiative, activeMap, activeMapId, journal, journalEntries, inventory, inventoryCoins, gameTime, calendarMonths, calendarEvents`. Глубина 20.

`undoLast` восстанавливает **только эти** поля. Не восстанавливаются: листы, чат, сцены, звук, `grid`, `scenes`, `timer`, `combat`, `partyNotes`, `rollRequests`, `reactions`, `showcase`, `vote`, `soundboard`, `adventure`, `mapPins` (пины внутри `maps` — восстанавливаются). После undo — `stateReload` всем.

Снимок делает не каждое действие: `uploadMap`, `deleteMap`, `setActiveMap`, `openMapFog`, `setMapFog`, `paintMapSurface`, `clearMapSurface`, `addMapTemplate`, `deleteMapTemplate`, `clearMapTemplates`, `tokensBatch`, `moveToken`, `moveTokenToActiveMap`, `moveAllTokensToActiveMap`, `setTokenHp` (при реальном изменении), `setTokenConditions`, `setTokenAura`, `setTokenVision`, `setCalendarMonths`, `addCalendarEvent`, `deleteCalendarEvent`, `setGameTime`, `saveJournal`, `saveJournalEntry`, `deleteJournalEntry`, `saveInventoryItem`, `deleteInventoryItem`, `setInventoryCoins`, `loadScene`. Не делают: `addMapWall`, `deleteMapWall`, `clearMapWalls`, `setMapWallDoor`, `addMapLight`, `setMapLight`, `deleteMapLight`, `setMapDarkness`, `setMapAutoLight`, `setMapFogFollow`, `setMapVision`, `setMapPin`, `deleteMapPin`, `setGrid`, инициатива, `partyLongRest`, `awardXp`, монеты листам.

### 5.10 Эфемерное и сохраняемое

- Не сохраняются: `vote`, `showcase`, `pendingReactions`, `undoStack`, `hpNotices`, `emote` (по TTL), `online`/`sockets`.
- Сохраняются: `players` (включая `playerKey`, `dmNote`), `sheets`, `maps` (с туманом, стенами, светом, шаблонами, пинами, `audioRef`), `activeMap`, `activeMapId`, `scenes`, `grid`, `gameTime`, `timer`, `calendarMonths`, `calendarEvents`, `partyNotes`, `tokens`, `initiative`, `combat`, `diceHistory`, `chatHistory`, `journal`, `journalEntries`, `inventory`, `inventoryCoins`, `rollRequests`, `soundboard`, `adventure`.

---

## 6. `disconnect` и повторный `joinRoom`

### disconnect

```js
const { role, playerId, name } = socket.data;
if (!role || !playerId) return;
room.detachSocket(playerId, socket.id);   // sockets.delete; online=false когда пусто
markDirty();
broadcastPlayers();
onLog(...)
```

- Ничего не удаляется: запись игрока, лист, фишки, реакции, заявки на броски, чат, `playerKey` остаются.
- Отдельного события «игрок вышел» нет: клиенты узнают по `playersUpdate` (`online: false`).
- Системного сообщения в чат нет.
- Таймеры (`hpNotices` 500 мс, снятие эмоута 6200 мс) продолжают работать.
- Повисшие `pendingReactions` и `rollRequests` адресата не чистятся.
- Сокет автоматически выходит из всех комнат.

### повторный join

1. Клиент шлёт `joinRoom` с тем же `playerId` и сохранённым `playerKey`.
2. `safeEqual` ключа → запись переиспользуется, `player.sockets.add`, `online = true`.
3. Приходит полный `joined` со `state`; клиент обязан заменить локальное состояние целиком (дельт нет).
4. Мультиокно: несколько сокетов одного игрока живут одновременно, все в комнате `playerId`, все получают `characterSheetData`, шёпот, реакции.
5. Повторный вход без ключа (старая кампания, игрок offline) допускается и закрепляет новый ключ за существующим `playerId`.
6. Повторный вход без ключа при `online = true` запрещён (`identity-denied`): второй ноутбук под тем же кодом не пустят, пока первый не отключится.
7. ДМ после перезапуска обязан прислать `hostKey`; ключ живёт только в памяти main-процесса и меняется при каждом `startHost`.
8. `disconnect` → `joinRoom` на **новом** сокете: старый сокет уже отвязан, состояние то же.

---

## 7. Где JS неоднозначен или сломан (не переносить вслепую)

Порядок: сначала то, что ломает данные или доступ, потом неоднозначности контракта.

1. **`journalImage` не ждёт сохранения файла.** `const saved = store.saveImage(...)` без `await`, дальше `/files/${saved}` → `"/files/[object Promise]"`. Запись журнала получает нерабочую ссылку. В порте — сохранять с ожиданием и проверять ошибку (сейчас `try/catch` отсутствует вовсе, rejection уйдёт в unhandled).
2. **Повторный `joinRoom` на том же сокете не запрещён.** Если клиент вызовет `joinRoom` второй раз с другим `playerId`, сокет останется в старой комнате игрока и получит чужие приватные события, а в `players[старый].sockets` останется мёртвый id → игрок навсегда `online`. Порт: либо отклонять второй join, либо сначала делать detach.
3. **`chatMessage` с несуществующим адресатом у ДМ деградирует в общее сообщение.** `to` остаётся `null`, и шёпот уходит `io.emit` всему столу. `chatImage` в той же ситуации возвращает ошибку. Порт: единое поведение — отказ.
4. **`deleteChatMessage` рассылает `chatDeleted {id}` всем.** Получатели, которым шёпот не был виден, узнают id и факт существования тайного сообщения. Безвредно для текста, но это утечка метаданных.
5. **`playSound` доступен любому joined.** Проверка только `requireJoined`, хотя комментарий и UI называют это кнопкой мастера. Любой игрок может запустить, зациклить или остановить звук у всего стола.
6. **`addMapWall` и `addMapLight` не кладут снимок в undo**, а `addMapTemplate` кладёт. Ctrl+Z после стены/света откатит более раннее действие, а не последнее.
7. **`undoLast` восстанавливает `initiative` из чужого снимка.** Действия инициативы не делают `pushUndo`, поэтому undo любой карты/фишки молча вернёт порядок боя, ход и раунд к моменту снимка. Также undo заменяет объекты токенов: таймеры (hp-уведомление, эмоут), держащие ссылку на старый объект, будут менять осиротевшие копии.
8. **`setAllTokensHidden` делает O(n) полных рассылок.** На каждую фишку `emitTokenVisibility` вызывает `markDirty` + `broadcastPlayers`; 200 фишек = 200 `playersUpdate` игрокам и 200 ДМ. Порт: одна рассылка на пачку.
9. **`announceTokenHp` склеивает удары неправильно.** Второй урон в окне 500 мс обновляет только таймер: `before` остаётся от первого удара (итоговая дельта верна), но `type`, `defense` и автор — от первого. Иммунитет второго удара теряется совсем, строка «иммунитет» не появится.
10. **`setMapPin` дублирует булавку при правке.** При найденном `index >= 0` элемент заменяется, а затем всё равно делается `item.pins.push(pin)` → в массиве две одинаковые записи. Лимит 60 при этом не соблюдается на этом пути.
11. **`setGrid` гасит сетку при частичном патче.** `normalizeGrid({...room.grid, ...data, enabled: Boolean(data.enabled)})`: если `enabled` не передан, сетка выключается. Клиент обязан всегда слать полный объект.
12. **Комментарий и код расходятся у `setGrid`.** Комментарий: «Сетку видят и двигают только он [мастер]». Код: `io.emit('gridUpdated', room.grid)` — сетку получают все, включая не присоединившихся.
13. **`io.emit` и `io.except` достигают сокетов без `joinRoom`.** Всё, что рассылается через `IO` (чат, броски, токены, `gridUpdated`, `soundCue` через `broadcast`, `tokensUpdated` через `io.except`), видит клиент, который ещё не представился. `snapshotFor` при этом не отдаётся. Порт: решить, нужен ли барьер «только `table`».
14. **`isHost` опирается на loopback, а игроки приходят через туннель iroh в тот же порт.** Если туннель терминируется локально, все игроки выглядят как `127.0.0.1`, и `hostKey` становится единственной защитой роли ДМ, а не «второй линией». В порте проверять адрес источника после туннеля или убрать это условие из модели угроз.
15. **`importSrd` не проверяет `room.adventure`.** У кампании без состояния (`RoomState` с `campaign.state == null`) `adventure === null`, и обработчик упадёт на `room.adventure.actors`. Остальные обработчики приключения такую проверку делают.
16. **`initiativeAdd` возвращает `null` и при дубле, и при лимите 40,** а текст всегда `Такой участник боя уже есть в списке`. Пользователь не узнает, что достигнут предел.
17. **`moveToken` доверяет клиентскому `path`.** `movedCells` растёт на `path.length - 1` (до 200 клеток за сообщение), сервер не сверяет путь с реальным смещением. Игрок может накрутить израсходованную скорость. Также `from`/`path` не проверяются на принадлежность карте.
18. **Автотуман в `moveToken` не попадает в undo:** `openMapFog` вызывается напрямую, без `pushUndo`, в отличие от одноимённого события.
19. **`hint` «пора поднять уровень» может не сработать при многократной выдаче.** Условие `sheet.experience >= nextThreshold && before < nextThreshold` вычисляется после начисления, но `before` — до; при нескольких получателях с одинаковым порогом в чат уйдёт по строке на каждого. Не баг, но шумно и не идемпотентно.
20. **`castSpell` не сверяет `slotLevel` с уровнем заклинания** (разрешён апкаст любым уровнем, включая уровень ниже), не проверяет `sheet.spells` на дубли и не снимает концентрацию. Сообщение в чате всегда «ячейка N уровня» либо «заговор».
21. **`rollDice` связывает бросок с заявкой по точному тексту `label`.** Правка формулировки заявки, две заявки с одинаковым текстом или клиент с другой подписью ломают автоснятие концентрации. Успешный спасбросок заявку не закрывает (закрывается только провал).
22. **`setTokenHidden` / `setAllTokensHidden` / `setTokenNameHidden` / `setTokenLabelsHidden` трактуют отсутствующее поле как «скрыть»:** `!(payload.hidden === false)`. Кривой payload скрывает фишку вместо ошибки. Решить явно: строгий boolean или дефолт.
23. **`tokensBatch` не валидирует `value` у `side`** (`value === 'ally' ? 'ally' : 'enemy'`) и у `hidden` (`value !== false`). Мусор молча становится «enemy»/«скрыть».
24. **`setTokenHp` сравнивает сырой payload с `sheet.hp`** (`Number(payload.hp) !== before`) для решения о `pushUndo`: строка `"12"` при равном значении даст лишний снимок.
25. **`playerList` не чистит никогда возвращавшихся.** Каждый, кто хоть раз вошёл, навсегда в списке участников (offline), и его лист сохраняется в кампании.
26. **`detachSocket` не снимает `pendingReactions` и адресные `rollRequests`.** Карточка реакции остаётся висеть у мастера до конца раунда.
27. **`deleteMap` не удаляет файл карты** — только запись; файл подчищается позже `sweepFiles` при сохранении.
28. **`loadScene` рассылает `stateReload` всем, даже при `show: false`.** Предпросмотр мастера стоит полного снимка каждому игроку.
29. **`showcaseSeen` и `setShowcase` не зовут `markDirty`.** Осознанно (шоу эфемерно), но `watchers` теряются при рестарте и вообще не попадают в кампанию.
30. **`voteCast` разрешён мастеру** (`requireJoined`), его голос попадает в `counts`, которые видят игроки; при этом `room.vote.votes` уходит только ДМ. Не описано, должен ли ДМ голосовать.
31. **`setAvatar` не проверяет `room.campaign`** перед `store.saveImage(room.campaign.id, ...)`; при кампании без id будет исключение вне `try`? Нет: вызов внутри `try`, но `room.campaign.id` вычисляется как аргумент внутри `try`, так что попадёт в `catch` и вернёт `Не получилось сохранить аватар`. То же у `uploadMap`, `uploadSound`, `setTokenImage`, `chatImage`.
32. **`emitToken('tokenMoved', token, meta)` передаёт явный payload,** поэтому фильтрация `tokenForViewer` для игроков не применяется — это правильно для координат, но означает, что любое новое поле в `meta` уйдёт игрокам без проверки.
33. **`token.hp` у фишки без `maxHp`** (`null`) не обновляется, а `applyTokenDamage` возвращает `null`; обработчик печатает `У фишки «…» нет прочности`, хотя причиной может быть и `amount = 0` (в `attackToken`/`monsterAttack` отдельной проверки суммы нет).
34. **`normalizeSheet` полностью заменяет объект листа.** Любой код, сохранивший ссылку на прежний объект (таймеры, замыкания), после `saveCharacterSheet` работает со старой копией. В порте это неважно, но при переносе логики «в лоб» — важно.
35. **`sanitizeText` режет по UTF-16 code units**, а не по кодпоинтам/графемам. Обрезка эмодзи даёт битую пару. В Rust `chars().take(n)` даст другое поведение — зафиксировать, какое считать контрактом (скорее всего кодпоинты).
36. **`close()` не снимает таймеры эмоутов** (снимает только `hpNotices`). После закрытия стола «хвостовой» `setTimeout` может дёрнуть `emitToken` по `io` уже закрытого сервера.
37. **`sessionLogExport` возвращает markdown целиком в ack** (без файла), а `saveJournal` ничего не рассылает — окно мастера обязано само обновить вид. Асимметрия с `journalEntries`, где есть `journalUpdated`.
38. **`rollTable` не пишет в `diceHistory`** и не логируется в сессию, в отличие от `rollDice`. История ленты кубиков после перезапуска не покажет таблицу.
39. **`setGameTime` не рассылает `calendarEventsUpdated`,** хотя `gameTime` и календарь в UI связаны; клиент пересчитывает сам.
40. **Дублирующиеся строки ошибок встречаются в разных доменах** (`Карта не найдена`, `Фишка не найдена`, `Игрок не найден`, `Только ДМ` vs `Это действие доступно только ДМ`). При порте не унифицировать: клиент может сравнивать текст.
41. **`createToken`, `createTokenFromAdventureActor` и `moveTokenToActiveMap` не рассылают `playersUpdate`,** хотя `createTokenFromSheet`, `assignTokenToPlayer`, `deleteToken` и `moveAllTokensToActiveMap` — рассылают. У мастера отметка «фишка игрока уже выведена» (поле `tokenId` в `playerList`) обновляется не сразу, а после любого следующего события, которое её пересобирает. В порте решить: `playersUpdate` после любого изменения привязки фишки к игроку.
