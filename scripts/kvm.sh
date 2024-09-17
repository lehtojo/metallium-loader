qemu-system-x86_64 $@ --enable-kvm \
    -machine q35 \
    -serial stdio --no-reboot -smp 2 -m 2048 \
    -drive if=pflash,format=raw,readonly=on,file=ovmf/OVMF_CODE.fd \
    -drive if=pflash,format=raw,readonly=on,file=ovmf/OVMF_VARS.fd \
    -drive format=raw,file=fat:rw:esp
