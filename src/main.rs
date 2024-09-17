#![feature(alloc_error_handler)]
#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use core::{alloc::Layout, fmt::Write, mem, panic::PanicInfo, ptr};
use uefi::{
    allocator, boot::{self, MemoryType}, fs::{FileSystem, FileSystemResult}, mem::memory_map::MemoryMap, prelude::entry, proto::{console::gop::GraphicsOutput, media::fs::SimpleFileSystem}, table::{
        boot::{BootServices, ScopedProtocol},
        cfg,
        Boot,
        SystemTable
    }, CString16, Handle, Status
};
use elfloader::*;

extern "C" {
    #[allow(improper_ctypes)]
    fn enter_kernel(entry: u64, info: *const BootInfo);
}

pub enum RegionKind {
    Unknown,
    Available,
    Reserved
}

pub struct Region {
    pub kind: RegionKind,
    pub start: u64,
    pub end: u64
}

impl Region {
    pub fn new(kind: RegionKind, start: u64, end: u64) -> Region {
        Region { kind, start, end }
    }
}

pub struct GraphicsInfo {
    framebuffer: usize,
    width: u32,
    height: u32,
    stride: u32
}

pub struct Regions {
    pub data: *const Region,
    pub length: usize
}

pub struct BootInfo {
    pub regions: Regions,
    pub kernel_regions: Regions,
    pub graphics: GraphicsInfo
}

impl Regions {
    pub fn empty() -> Regions {
        Regions {
            data: ptr::null(),
            length: 0
        }
    }

    pub fn new(data: *const Region, length: usize) -> Regions {
        Regions { data, length }
    }
}

impl BootInfo {
    fn new() -> BootInfo {
        BootInfo {
            regions: Regions::empty(),
            kernel_regions: Regions::empty(),
            graphics: GraphicsInfo::new()
        }
    }
}

impl GraphicsInfo {
    fn new() -> GraphicsInfo {
        GraphicsInfo { framebuffer: 0, width: 0, height: 0, stride: 0 }
    }
}

struct KernelLoader {
    base: u64,
    system_table: SystemTable<Boot>,
    regions: Vec<Region>
}

impl ElfLoader for KernelLoader {
    fn allocate(&mut self, load_headers: LoadableHeaders) -> Result<(), ElfLoaderErr> {
        let stdout = self.system_table.stdout();

        for header in load_headers {
            writeln!(
                stdout,
                "Program header: address = {:#x}, size = {:#x}, flags = {}",
                header.virtual_addr(),
                header.mem_size(),
                header.flags()
            ).unwrap();
        }

        Ok(())
    }

    fn relocate(&mut self, entry: RelocationEntry) -> Result<(), ElfLoaderErr> {
        use RelocationType::x86_64;
        use crate::arch::x86_64::RelocationTypes::*;

        let stdout = self.system_table.stdout();
        let address: *mut u64 = (self.base + entry.offset) as *mut u64;

        match entry.rtype {
            x86_64(R_AMD64_RELATIVE) => {
                let addend = entry.addend
                    .ok_or(ElfLoaderErr::UnsupportedRelocationEntry)?;
                let value = self.base + addend;

                writeln!(
                    stdout,
                    "Relocation: AMD64_RELATIVE: *{:p} = {:#x}",
                    address,
                    value
                ).unwrap();

                unsafe {
                    *address = value;
                }

                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn load(&mut self, _flags: Flags, base: VAddr, region: &[u8]) -> Result<(), ElfLoaderErr> {
        let stdout = self.system_table.stdout();
        let start = self.base + base;
        let end = self.base + base + region.len() as u64;
        writeln!(stdout, "Loading program header into {:#x}-{:#x}", start, end).unwrap();

        // Reserve the region for the kernel
        self.regions.push(Region::new(RegionKind::Reserved, start, end));

        unsafe {
            ptr::copy_nonoverlapping(region.as_ptr(), start as *mut u8, region.len());
        }

        Ok(())
    }

    fn tls(
        &mut self,
        _tls_data_start: VAddr,
        _tls_data_length: u64,
        _total_size: u64,
        _align: u64
    ) -> Result<(), ElfLoaderErr> {
        let stdout = self.system_table.stdout();
        writeln!(stdout, "Kernel TLS sections are not supported").unwrap();
        Err(ElfLoaderErr::UnsupportedSectionData)
    }
}

fn read_file(boot_services: &BootServices, path: &str) -> FileSystemResult<Vec<u8>> {
    let path: CString16 = CString16::try_from(path).unwrap();
    let filesystem_protocol: ScopedProtocol<SimpleFileSystem> = boot_services
        .get_image_file_system(boot_services.image_handle())
        .unwrap();
    let mut filesystem = FileSystem::new(filesystem_protocol);
    filesystem.read(path.as_ref())
}

fn load_kernel_blob(boot_services: &BootServices) -> Vec<u8> {
    read_file(boot_services, "efi\\boot\\kernel")
        .expect("Failed to load kernel binary blob")
}

fn load_kernel(system_table: SystemTable<Boot>, info: &mut BootInfo) -> u64 {
    let boot_services = system_table.boot_services();
    let blob = load_kernel_blob(boot_services);
    let binary = ElfBinary::new(blob.as_slice())
        .expect("Failed to parse kernel binary");

    let mut loader = KernelLoader { base: 0, system_table, regions: Vec::new() };
    binary.load(&mut loader).expect("Failed to load kernel");

    let (data, length) = (loader.regions.as_ptr(), loader.regions.len());
    mem::forget(loader.regions);
    info.kernel_regions = Regions::new(data, length);

    binary.file.header.pt2.entry_point()
}

fn load_regions(system_table: &SystemTable<Boot>) -> Regions {
    let memory_map = system_table.boot_services().memory_map(MemoryType::LOADER_DATA)
        .expect("Failed to load memory map");

    let mut regions = Vec::new();

    for descriptor in memory_map.entries() {
        let kind = match descriptor.ty {
            MemoryType::CONVENTIONAL => RegionKind::Available,
            _ => RegionKind::Reserved
        };
        let start = descriptor.phys_start;
        let end = start + descriptor.page_count * 0x1000;

        regions.push(Region::new(kind, start, end));
    }

    // Steal the region data to ourselves, so that we can pass it to the kernel
    let (data, length) = (regions.as_ptr(), regions.len());
    core::mem::forget(regions);

    Regions::new(data, length)
}

#[entry]
unsafe fn main(
    image: Handle,
    mut system_table: SystemTable<Boot>,
) -> Status {

    allocator::init(&mut system_table);

    // Load standard output handle for printing information
    let mut system_table_clone = system_table.unsafe_clone();
    let stdout = system_table_clone.stdout();
    stdout.clear().unwrap();
    writeln!(stdout, "Loading kernel...").unwrap();

    // Note: Debugging code if everything goes wrong
    // writeln!(stdout, "Testing allocation...").unwrap();
    // let mut vector: Vec<u32> = Vec::new();
    // vector.push(1);
    // vector.push(2);
    // writeln!(stdout, "Vector = {:?}", vector).unwrap();

    // Find RSDP for finding information of the system
    let mut config_entries = system_table.config_table().iter();
    let rsdp_address = config_entries
        .find(|entry| matches!(entry.guid, cfg::ACPI_GUID | cfg::ACPI2_GUID))
        .map(|entry| entry.address)
        .expect("Failed to find RSDP address");
    writeln!(stdout, "RSDP address: {:?}", rsdp_address).unwrap();

    writeln!(stdout, "Loading memory information...").unwrap();
    let mut info = BootInfo::new();
    info.regions = load_regions(&system_table);

    writeln!(stdout, "Loading kernel into memory...").unwrap();
    let entry = load_kernel(system_table.unsafe_clone(), &mut info);

    // Todo: Remember to also reserve kernel load region

    writeln!(stdout, "Kernel is now in memory!").unwrap();
    writeln!(stdout, "Kernel entry: {:#X}", entry).unwrap();

    // Load GOP information for displaying graphics in the kernel
    let gop_handle = system_table
        .boot_services()
        .get_handle_for_protocol::<GraphicsOutput>()
        .expect("Failed to get GOP protocol handle");

    let mut gop = system_table
        .boot_services()
        .open_protocol_exclusive::<GraphicsOutput>(gop_handle)
        .expect("Failed to open GOP protocol");

    let mode_info = gop.current_mode_info();
    let (width, height) = mode_info.resolution();
    let stride = mode_info.stride() as u32 * 4;
    let framebuffer = gop.frame_buffer().as_mut_ptr() as usize;

    writeln!(stdout, "GOP mode: {:?}", mode_info).unwrap();
    writeln!(stdout, "GOP framebuffer address: {:#X}", framebuffer) .unwrap();
    info.graphics.framebuffer = framebuffer;
    info.graphics.width = width as u32;
    info.graphics.height = height as u32;
    info.graphics.stride = stride;

    writeln!(stdout, "GOP framebuffer: width={}, height={}, stride={}", width, height, stride).unwrap();

    writeln!(stdout, "Main is at {:#p}", main as *const u8).unwrap();
    writeln!(stdout, "Entering kernel via {:#p}", enter_kernel as *const u8).unwrap();

    // let mmap_storage = {
    //     let max_mmap_size =
    //         system_table.boot_services().memory_map_size() + 8 * mem::size_of::<MemoryDescriptor>();
    //     let ptr = system_table
    //         .boot_services()
    //         .allocate_pool(MemoryType::LOADER_DATA, max_mmap_size)?
    //         .unwrap();
    //     unsafe { slice::from_raw_parts_mut(ptr, max_mmap_size) }
    // };

    // We no longer need any boot services, we're ready to enter the kernel
    let _ = boot::exit_boot_services(MemoryType::BOOT_SERVICES_DATA);

    // let (system_table, memory_map) = system_table
    //     .exit_boot_services(image, mmap_storage)
    //     .unwrap()
    //     .unwrap();

    enter_kernel(entry, &info);
    loop {}
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

#[alloc_error_handler]
fn alloc_error(_layout: Layout) -> ! {
    panic!("Out of memory")
}
