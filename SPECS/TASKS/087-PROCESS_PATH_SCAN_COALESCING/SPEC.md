# SPEC: 087 — PROCESS_PATH_SCAN_COALESCING

**Фича:** [HOTFIXES](../../FEATURES/004-HOTFIXES/FEATURE.md)

| Поле | Значение |
|------|----------|
| Тип | R — CPU существующего Linux/Android owner-query |
| Статус | C — source/test/release-кандидат проверен; production deployment не выполнен |
| Base | `871b7a3440cd9c850f1b09ef3b4ada9babb4384b` |

## Контракт

При `linux && with_ebpf` одновременные cache misses для одного UID
сериализуются 64 фиксированными lock-shards. После получения lock
повторно проверяется существующий cache. Снимок используется только
при наличии запрошенного socket inode; новый inode требует нового scan.

TTL, полный список executable paths, netlink UID/package resolution,
fallback на все UID, ошибки и маршрутизация сохраняются. Cache hit
возвращается до нового lock. Между UID и all-users fallback lock не
удерживается. Новых таймеров, настроек, горутин и растущих UID-lock maps нет.
Без `with_ebpf` stub сохраняет исходный порядок cache/build/cache-add.

## Измерение

Изолированный тестовый процесс на физическом Android ARM64, Go 1.26.7,
CGO=0; оба алгоритма в одном executable и с долгоживущим searcher.
Шесть прогонов, порядок reference/candidate чередуется. Измеряются
реальные локальные socket inode и `/proc`, а не mock scan.

Медиана CPU на burst: 4 callers — 30.319 → 7.708 ms (-74.58%);
8 callers — 57.689 → 8.091 ms (-85.97%). Один caller: 9.490 →
9.435 ms, практически без изменения. Это измерение одной процедуры
для root-owned сокетов, не снижение CPU всего ядра или расхода батареи.

Первичные прогоны с отдельными процессами и созданием searcher для
каждого burst сохранены в отчётах. Их single-caller разброс не принят
как доказательство регрессии/ускорения постоянного production searcher.

## Приёмка

- Concurrent callers получают действительные пути существующих sockets.
- Новый inode не теряется из-за ранее заполненного cache.
- Сохраняется all-users fallback при несовпадающем UID.
- Существующие process tests, race/vet, no-tag и Android ARM64 builds проходят.
- Dependency patch p2p1 сохраняется; настройки и запущенное ядро не заменяются.

## Отклонено

Полностью отключать executable-path lookup при package-only правилах:
это изменило бы диагностические поля и усложнило совместимость с живыми
rule-set updates. Оптимизация объединяет лишнюю работу и сохраняет
полный результат вместо изменения этих контрактов.

Снятие: заменить, когда базовая реализация сама координирует UID scans.
