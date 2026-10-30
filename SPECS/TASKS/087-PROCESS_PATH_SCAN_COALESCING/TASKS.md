# TASKS — 087

- [x] Ограниченные UID lock-shards и no-tag stub.
- [x] Cache recheck по целевому inode; прежний fallback и TTL.
- [x] Host process tests, race/vet и no-tag tests.
- [x] Android ARM64 functional tests изолированного executable.
- [x] Шесть чередующихся same-binary измерений CPU/allocations на устройстве.
- [x] Удаление временных тестовых файлов; проверка неизменности live core/config.
- [x] Release-сборки и p2p1 dependency regression.
- [x] Экспорт патча и итоговый отчёт.

CPU1 установлен отдельной операцией. Rust live deployment и измерение
расхода батареи остаются отдельной приёмкой; Rust эксперимент не заменяет ядро телефона.

## Эксперимент Rust

- [x] Build-tag backend и явный CGO opt-in без новых upstream seams.
- [x] Native ownership/FFI/error/UID проверки и Go algorithm control.
- [x] Same-binary и interleaved Android измерения.
- [x] Полный experimental ARM64 core и check действующего конфига.
- [x] Проверить неизменность production core, policy, 34 selectors и очистить staging.
