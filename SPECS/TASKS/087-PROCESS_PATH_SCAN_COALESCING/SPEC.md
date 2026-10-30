# SPEC: 087 — PROCESS_PATH_SCAN_COALESCING

**Фича:** [HOTFIXES](../../FEATURES/004-HOTFIXES/FEATURE.md)

| Поле | Значение |
|------|----------|
| Тип | R — CPU существующего Linux/Android owner-query |
| Статус | C — CPU1 + Rust1 установлен на целевом Android; энергопотребление не измерено |
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
Без `with_ebpf` stub сохраняет исходный порядок cache/build/cache-add;
выбор Rust backend управляется отдельным тегом и не включает coalescing.

## Опциональный Rust backend

По прямому запросу пользователя Linux/Android scanner получает отдельный
экспериментальный backend `with_lx_rust_proc && cgo`. По умолчанию тег
отсутствует и остаётся Go scanner CPU1. Rust1 установлен отдельной операцией по явному запросу пользователя.

Rust выполняет полный scan за один C ABI вызов; общий UID cache, TTL,
64 lock-shards, inode recheck, all-users fallback и package resolution не
меняются. Результат содержит все inode и все уникальные executable paths
в прежнем порядке, включая произвольные Unix path bytes. Ошибки scan не
маскируются частичным результатом. Rust владеет сериализованным buffer;
Go копирует строки и вызывает парный Rust free. Go pointers не сохраняются,
Rust unwind не пересекает C ABI. Бесконечных фоновых worker нет.

Тег требует Rust staticlib и `LX_CGO_ENABLED=1`. Default Makefile остаётся
CGO=0. Для Android эксперимент явно добавляет `netgo,osusergo`, чтобы
не переключать стандартный DNS/user lookup на libc. Сборки с тегом и без
него проверяются отдельно. Рабочий eBPF/mode/node/config не меняется.

Точная новая поверхность upstream: **0 файлов**; используются существующий
lx helper `common/process/lx_process_path_scan_shared_linux.go`, новые
lx файлы, crate `experimental/lx_rust_proc`, параметр в `Makefile.lx`.
Это один backend той же задачи 087, не отдельная дублирующая спецификация.
Приёмка сравнивает CPU1, Rust через FFI и аналогичную Go оптимизацию;
Go heap metrics не считаются полной памятью Rust. Функциональные тесты
и `check` не доказывают экономию всей батареи или live-трафик нового ядра.

## Измерение CPU1

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

## Измерение Rust

Проверка на физическом Android ARM64: 864 полных scan в одном executable,
6 блоков по 24 цикла для UID и all-users, каждый цикл чередует CPU1 Go,
buffered Go control и Rust через C ABI. Порядок сбалансирован; учитываются
FFI, сериализация, Go decode и process CPU. Для UID paired median CPU
сократился на 12.49%, для all-users — на 12.38%. Во всех шести блоках
Rust быстрее; по циклам 140/144 и 144/144 соответственно. Go control
даёт 8.84% и 6.41%: часть выигрыша объясняется алгоритмом/аллокациями.
Ранние длинные последовательные серии дали больший UID выигрыш и
all-users выбросы, поэтому основное сравнение — interleaved.

Собран полный ARM64 кандидат `.p2p1.cpu1.rust1`; на телефоне проходят
16 integrated process tests и `check` неизменённого рабочего конфига.
CPU1 PID/core/config, 31 policy file и 34 selector сохранены. Это не
измерение общего CPU, RSS или расхода батареи; Rust1 установлен отдельно; live DNS/HTTPS/eBPF и сохранность config/selector проверены.

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
