# PLAN — 087

1. Сохранить существующий быстрый cache-hit и TTL.
2. На cache miss использовать фиксированный per-searcher lock-shard по UID.
3. После lock проверить именно запрошенный inode; иначе выполнить прежний scan.
4. Вынести механизм в Linux lx-файлы с `with_ebpf` и no-tag stub.
5. Проверить concurrent/new-inode/all-users случаи, race/vet и ARM64.
6. Сравнить CPU и allocations на реальных sockets одного Android устройства.
7. Сохранить p2p1 dependency patch и собрать изолированный release-кандидат.

Зона upstream-конфликта: только `common/process/searcher_linux.go`, поле
searcher и вызов build/cache publication. Внешние API и правило выбора
процесса не меняются.
