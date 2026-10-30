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
