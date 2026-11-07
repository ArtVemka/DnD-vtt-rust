# Очередь порта `server/state.js`

Отметки: `[x]` готово, `[~]` в работе, `[ ]` не начато.

## Готово

- [x] `LIMITS`, `COIN_KEYS`, `ABILITY_KEYS`, `ABILITY_LABELS`, `SKILLS` (`sheet.rs`)
- [x] `clampInt`, `clampNumber`, `sanitizeText` (`sheet.rs`)
- [x] `createEmptySheet`, `emptyCoins`, `normalizeCoins`, `normalizeSheet`,
  `normalizeSheetClasses`, `normalizeAttackList`, `normalizeSheetSpells` (`sheet.rs`)
- [x] `normalizeStatblock`, `normalizeAdventure`, `normalizeAdventureImage`,
  `normalizeImageCrop` (`sheet.rs`)
- [x] `CAMPAIGN_FORMAT_VERSION`, `MAP_TYPES`, `emptyMaps` (`model.rs`)
- [x] `DICE_SIDES`, формулы и броски (`dice.rs`)

## Осталось

- [ ] `TIME_LIMITS`, `DEFAULT_GAME_TIME`, `emptyTime`, `normalizeTime`
- [ ] `emptyTimer`, `normalizeTimer`, `normalizeCalendarMonths`
- [ ] `INITIATIVE`, `INITIATIVE_KINDS`, `emptyInitiative`, `normalizeInitiative`
- [ ] `normalizeConditions`, `normalizeConditionDurations`, `normalizeConditionUnits`
- [ ] `normalizeAura`, `normalizeVision`, `normalizeDamageDefenses`
- [ ] `normalizeFog`, `normalizeTokenEmote`, `normalizePins`, `normalizeDarkness`,
  `normalizeTokenLight`
- [ ] `TEMPLATE_KINDS`, `TEMPLATE_LIMITS`, `normalizeTemplate`, `normalizeTemplates`
- [ ] `JOURNAL_KINDS`, `normalizeJournalEntry`, `normalizeJournalEntries`
- [ ] `normalizeInventoryItem`, `normalizeInventory`
- [ ] `normalizeRollRequest`, `normalizeRollRequests`
- [ ] `normalizeAudioRef`, `normalizeSound`, `normalizeSoundboard`
- [ ] `normalizeCalendarEvent`, `normalizeCalendarEvents`
- [ ] генераторы id: `newTokenId`, `newMapId`, `newSceneId`, `newJournalId`,
  `newInventoryId`, `newCalendarEventId`, `newRollRequestId`, `newSoundId`
- [ ] `abilityModifier`, `proficiencyBonus` (если ещё не в `rules.rs`)

## RoomState (методы)

Порядок переноса:

1. `constructor`, `hydrate`, `serialize`, `snapshotFor`, `playerList`
2. `addPlayer`, `addDm`, `renamePlayer`, `detachSocket`, `isOnline`
3. `normalizeMapSlot`, `currentMap`, `isMapPublished`, `isTokenVisibleToPlayers`,
   `mapSlotForViewer`, `mapsForViewer`, `normalizeToken`, `tokenForViewer`
4. инициатива, раунды, реакции, условия
5. фишки: hp, урон, лечение, защита, концентрация, аура, зрение
6. сцены: `normalizeScenes`, `captureScene`, `applyScene`
7. журнал, склад, календарь, кошелёк, заметки
8. голосование, таймер, showcases, звук, undo

## Известные хвосты

- `normalizeSheet` не применяет финальный кламп `hp` по `effectiveMaxHp`:
  функция живёт в `rules.rs` и появится вместе с ним.
- `sanitizeText` режет по UTF-16, как JS, но не рвёт суррогатную пару.
- В `session_log.rs` строки markdown соединяются через `\n`; в JS они
  склеивались без разделителя, это был баг.
