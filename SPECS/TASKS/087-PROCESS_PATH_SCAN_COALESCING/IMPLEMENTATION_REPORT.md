# IMPLEMENTATION REPORT — 087

## Изменения

- Upstream seam: `common/process/searcher_linux.go`, bounded locks в searcher,
  делегирование cache miss; блоки `// lx:`.
- Новые файлы `lx_process_path_scan_{linux,stub_linux,shared_linux}.go`:
  coordination под `with_ebpf`, исходный no-tag путь, общая публикация cache.
- `lx_process_path_scan_linux_test.go`: реальные UDP inode, parallel calls,
  новый inode, all-users fallback; cross-binary и same-binary benchmarks.
- Новых пользовательских опций, публичных JSON/state полей, DNS/routing
  политик, таймеров и фоновых workers нет.

## Проверка

- Go 1.26.7: process/route/protocol-ebpf/common-ebpf tests проходят.
- Process tests проходят с `-race`, с тегом и без него; vet проходит.
- Полная Android ARM64 release-сборка с исходным feature set проходит.
- Изолированная p2p1 dependency сохраняет zero-headroom fix;
  `TestLXWireGuardSendHeadroom` проходит.
- Кандидат на реальном Android принимает текущий полный config через
  `sing-box check`; активное ядро и config не заменены, сервис ready.
- Шесть same-binary прогонов с чередованием порядка: CPU на 4/8 concurrent
  lookups -74.58%/-85.97%, один lookup практически без изменения.
- Временные device-файлы удалены; profiler не остаётся attached.

Plain `go build ./...` выявляет существующую ошибку forbidden linkname в
`experimental/boxdd`, одинаковую на неизменённом base. С документированным
`-ldflags=-checklinkname=0` полный no-tag build проходит; release использует
канонический `Makefile.lx`, уже содержащий этот флаг.

## Границы

Это CPU/allocations одной процедуры на root-owned sockets при холодном
cache. Уменьшение CPU всего production ядра, расхода батареи, задержки
браузера и поведение после замены на телефоне не измерены. Cache hit и
долгоживущие потоки не получают заявленного 75–86% ускорения.

Первичные короткоживущие cross-binary результаты сохранены отдельно;
основная оценка использует один executable и постоянный searcher.

Rebase surface: одно upstream поле и один участок build/cache publication
в `common/process/searcher_linux.go`. Новые lx-файлы переносятся отдельно.

## Дополнение: opt-in Rust scanner, 2026-10-30

По запросу пользователя создан отдельный worktree от CPU1 `bc97aa4`.
Добавлены crate `experimental/lx_rust_proc`, C ABI bridge/free/decoder,
`with_lx_rust_proc` и Go stub. Из нового upstream diff — 0 файлов;
проводка затрагивает только lx helper и `Makefile.lx` (CGO default 0).
Включение Rust требует LX_CGO_ENABLED=1 и staticlib; netgo/osusergo
сохраняют Go DNS/user lookup. Путь Go cache-hit не меняется.

Проверены raw Unix bytes, dedup/order, Unicode digits, UID/all-users,
исчезающие/недоступные entries, scan errors, native socket ownership,
concurrent FFI/free и malformed snapshot. Три Rust unit tests, Go race/vet,
strict cgocheck2 и decoder fuzz (755924 inputs) проходят. Встроенный
process suite: 16 тестов на Android; также host route/ebpf и no-tag
build ./... с обязательным -checklinkname=0.

Основное измерение: 864 полных /proc scans, 6 interleaved блоков.
Paired median CPU: UID -12.49%, all-users -12.38%. Go algorithm control:
-8.84%/-6.41%. Все 6 блоков Rust быстрее; это CPU отдельной процедуры,
не CPU всего ядра и не батарея. Go allocations не учитывают Rust heap.

Полный `.p2p1.cpu1.rust1` собран каноническим Makefile.lx с сохранённым
p2p1 dependency patch. На телефоне кандидат прошёл version/tag и check
рабочего конфига; не запускался как network service. Работающий CPU1
PID 13310 и SHA/config/31 policy/34 selectors сохранены; health healthy,
DNS ok, Tailscale identity SHA до/после совпадает. Все staging скрипты
завершили очистку. После финального fingerprint ADB отсоединился,
поэтому дополнительная повторная inventory-проверка не выполнялась.

Артефакты и сырые метрики: ../reports в корне experiment workspace
`/root/ChatGPT/magicnet/work/rust-proc-scan-20261030`.
