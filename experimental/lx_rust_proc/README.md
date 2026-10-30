# Экспериментальный Rust backend process scanner

Backend задачи [087](../../SPECS/TASKS/087-PROCESS_PATH_SCAN_COALESCING/SPEC.md).
Не включён по умолчанию. Сборка без `with_lx_rust_proc` использует исходный
Go scanner и не зависит от Rust/CGO. Новых runtime настроек нет.

```sh
./experimental/lx_rust_proc/build.sh aarch64-linux-android
# CC должен указывать на NDK clang соответствующего Android API.
GOOS=android GOARCH=arm64 make -f Makefile.lx lx-build \
  LX_CGO_ENABLED=1 LX_TAGS="<прежние теги>,with_lx_rust_proc,netgo,osusergo"
```

Для host tests сначала собрать target `x86_64-unknown-linux-gnu`.
Тег с CGO=0 намеренно не имеет fallback и не собирается: запрос Rust
не должен молча выпускать Go backend. Staticlib/target не коммитятся.

C ABI принимает root bytes только на время вызова. Rust возвращает свой
buffer; Go валидирует формат, копирует строки, вызывает Rust free.
Inode u32, unique executable paths и их порядок сохраняются. Paths не
преобразуются через UTF-8. Panic перехватывается до C ABI, scan error
возвращает errno. Fatal OOM не является восстанавливаемым контрактом.

`src/unicode_digits.rs` сгенерирован `gen-unicode.go` из `unicode.Digit`
закреплённого Go 1.26.7. Обновление Go Unicode tables требует явной
регенерации; обычная Rust сборка источник не меняет.
