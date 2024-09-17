clang -c boot.s -o boot.o -target x86_64-unknown-windows -ffreestanding -mno-red-zone -fshort-wchar
ar rcs boot.lib boot.o
