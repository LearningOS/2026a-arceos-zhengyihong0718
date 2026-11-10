#![cfg_attr(feature = "axstd", no_std)]
#![cfg_attr(feature = "axstd", no_main)]
#![feature(asm_const)]
#![feature(riscv_ext_intrinsics)]

#[cfg(feature = "axstd")]
extern crate axstd as std;
extern crate alloc;
#[macro_use]
extern crate axlog;

mod task;
mod vcpu;
mod regs;
mod csrs;
mod sbi;
mod loader;

use vcpu::VmCpuRegisters;
use riscv::register::{scause, sstatus, stval};
use csrs::defs::hstatus;
use tock_registers::LocalRegisterCopy;
use csrs::{RiscvCsrTrait, CSR};
use vcpu::_run_guest;
use sbi::SbiMessage;
use loader::load_vm_image;
use axmm::AddrSpace;
use axhal::mem::{PhysAddr, VirtAddr};
use crate::regs::GprIndex::{A0, A1};

const VM_ENTRY: usize = 0x8020_0000;

#[cfg_attr(feature = "axstd", no_mangle)]
fn main() {
    ax_println!("Hypervisor ...");

    // A new address space for vm.
    let mut uspace = axmm::new_user_aspace().unwrap();

    // Load vm binary file into address space.
    if let Err(e) = load_vm_image("/sbin/skernel2", &mut uspace) {
        panic!("Cannot load app! {:?}", e);
    }

    // Setup context to prepare to enter guest mode.
    let mut ctx = VmCpuRegisters::default();
    prepare_guest_context(&mut ctx);

    // Setup pagetable for 2nd address mapping.
    let ept_root = uspace.page_table_root();
    prepare_vm_pgtable(ept_root);

    // Kick off vm and wait for it to exit.
    while !run_guest(&mut ctx, &uspace) {
    }

    panic!("Hypervisor ok!");
}

fn prepare_vm_pgtable(ept_root: PhysAddr) {
    let hgatp = 8usize << 60 | usize::from(ept_root) >> 12;
    unsafe {
        core::arch::asm!(
            "csrw hgatp, {hgatp}",
            hgatp = in(reg) hgatp,
        );
        core::arch::riscv64::hfence_gvma_all();
    }
}

fn run_guest(ctx: &mut VmCpuRegisters, uspace: &AddrSpace) -> bool {
    // Keep host interrupts disabled while switching the CSR state and entering
    // the guest. Otherwise a timer interrupt can arrive after `stvec` points at
    // `_guest_exit` but before `sscratch` holds the GuestInfo pointer.
    let flags = axhal::arch::local_irq_save_and_disable();
    unsafe {
        _run_guest(ctx);
    }
    let stop = vmexit_handler(ctx, uspace);
    axhal::arch::local_irq_restore(flags);
    stop
}

#[allow(unreachable_code)]
fn vmexit_handler(ctx: &mut VmCpuRegisters, uspace: &AddrSpace) -> bool {
    use scause::{Exception, Trap};

    let scause = scause::read();
    match scause.cause() {
        Trap::Exception(Exception::VirtualSupervisorEnvCall) => {
            let sbi_msg = SbiMessage::from_regs(ctx.guest_regs.gprs.a_regs()).ok();
            ax_println!("VmExit Reason: VSuperEcall: {:?}", sbi_msg);
            if let Some(msg) = sbi_msg {
                match msg {
                    SbiMessage::Reset(_) => {
                        let a0 = ctx.guest_regs.gprs.reg(A0);
                        let a1 = ctx.guest_regs.gprs.reg(A1);
                        ax_println!("a0 = {:#x}, a1 = {:#x}", a0, a1);
                        assert_eq!(a0, 0x6688);
                        assert_eq!(a1, 0x1234);
                        ax_println!("Shutdown vm normally!");
                        return true;
                    },
                    _ => todo!(),
                }
            } else {
                panic!("bad sbi message! ");
            }
        },
        Trap::Exception(Exception::IllegalInstruction) => {
            // The guest reads `mhartid`, which is not available in VS-mode.
            // Emulate `csrr rd, mhartid` with a stable virtual hart id.
            let pc = ctx.guest_regs.sepc;
            ax_println!("VmExit: illegal instruction at {:#x}", pc);
            if let Some(insn) = read_guest_insn(uspace, pc) {
                if insn & 0x7f == 0x73
                    && (insn >> 12) & 0x7 == 0x2
                    && (insn >> 20) & 0xfff == 0xf14
                {
                    let rd = (insn >> 7) & 0x1f;
                    if let Some(reg) = crate::regs::GprIndex::from_raw(rd) {
                        ctx.guest_regs.gprs.set_reg(reg, 0x1234);
                    }
                    ctx.guest_regs.sepc = pc.wrapping_add(4);
                    return false;
                }
            }
            panic!("Bad instruction: {:#x} sepc: {:#x}",
                stval::read(),
                ctx.guest_regs.sepc
            );
        },
        Trap::Exception(Exception::LoadGuestPageFault) => {
            // The guest's boot ROM load at GPA 0x40 is emulated: complete the
            // `ld` instruction with the virtual boot-data word and skip it.
            let pc = ctx.guest_regs.sepc;
            let fault_addr = stval::read();
            ax_println!("VmExit: guest page fault at {:#x}, pc {:#x}", fault_addr, pc);
            if fault_addr == 0x40 {
                if let Some(insn) = read_guest_insn(uspace, pc) {
                    if insn & 0x7f == 0x03
                        && (insn >> 12) & 0x7 == 0x3
                        && (insn >> 15) & 0x1f == 0
                        && ((insn as i32) >> 20) as usize == 0x40
                    {
                        let rd = (insn >> 7) & 0x1f;
                        if let Some(reg) = crate::regs::GprIndex::from_raw(rd) {
                            ctx.guest_regs.gprs.set_reg(reg, 0x6688);
                        }
                        ctx.guest_regs.sepc = pc.wrapping_add(4);
                        return false;
                    }
                }
            }
            panic!("LoadGuestPageFault: stval{:#x} sepc: {:#x}",
                fault_addr,
                ctx.guest_regs.sepc
            );
        },
        _ => {
            panic!(
                "Unhandled trap: {:?}, sepc: {:#x}, stval: {:#x}",
                scause.cause(),
                ctx.guest_regs.sepc,
                stval::read()
            );
        }
    }
    false
}

fn read_guest_insn(uspace: &AddrSpace, pc: usize) -> Option<u32> {
    let mut buf = [0u8; 4];
    uspace.read(VirtAddr::from(pc), &mut buf).ok()?;
    Some(u32::from_le_bytes(buf))
}

fn prepare_guest_context(ctx: &mut VmCpuRegisters) {
    // Set hstatus
    let mut hstatus = LocalRegisterCopy::<usize, hstatus::Register>::new(
        riscv::register::hstatus::read().bits(),
    );
    // Set Guest bit in order to return to guest mode.
    hstatus.modify(hstatus::spv::Guest);
    // Set SPVP bit in order to accessing VS-mode memory from HS-mode.
    hstatus.modify(hstatus::spvp::Supervisor);
    CSR.hstatus.write_value(hstatus.get());
    ctx.guest_regs.hstatus = hstatus.get();

    // Set sstatus in guest mode.
    let mut sstatus = sstatus::read();
    sstatus.set_spp(sstatus::SPP::Supervisor);
    // The guest only performs synchronous SBI calls; keep interrupts off there
    // so they cannot preempt the tiny guest before it issues its `ecall`.
    // `sret` copies SPIE into SIE, so both bits must be cleared.
    ctx.guest_regs.sstatus = sstatus.bits() & !0b10_0010;
    // Return to entry to start vm.
    ctx.guest_regs.sepc = VM_ENTRY;
}
