set arch i386:x86-64:intel
target remote localhost:1234
add-symbol-file ./esp/efi/boot/kernel 0xffff800000207000
directory ../metallium/
b _start