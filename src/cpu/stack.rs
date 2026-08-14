use crate::cpu::core::Cpu;
use crate::cpu::registers::CpuFlag::{C, Z};

/// Push a 16-bit value the way hardware does: **high byte first** at `SP-1`,
/// then the low byte at `SP-2`, one M-cycle each.
///
/// The order is observable. Mooneye's `acceptance/rst_timing`,
/// `call_timing`, `push_timing` and friends DMA a known byte into OAM, point
/// `SP` there and then run the instruction, so whichever push lands while the
/// DMA still owns OAM is lost — writing the low byte first swaps exactly the
/// two bytes those tests compare. It also matters for `SP = $0000`, where the
/// first push wraps onto `IE` (`acceptance/interrupts/ie_push`).
pub fn pushstack(cpu: &mut Cpu, value: u16) {
    cpu.registers.sp = cpu.registers.sp.wrapping_sub(1);
    cpu.memory.wb(cpu.registers.sp, (value >> 8) as u8);
    cpu.registers.sp = cpu.registers.sp.wrapping_sub(1);
    cpu.memory.wb(cpu.registers.sp, value as u8);
}

/// Pop a 16-bit value: low byte from `SP`, high from `SP+1`, one M-cycle each.
///
/// Both M-cycles carry a read *and* an IDU increment, but only the first
/// asserts the IDU's glitched write — Pan Docs records `pop` as triggering the
/// DMG OAM bug three times rather than the four its cycles suggest, "one read,
/// one glitched write, and another read without a glitched write".
fn popstack(cpu: &mut Cpu) -> u16 {
    cpu.memory.share_idu(cpu.registers.sp);
    let lo = cpu.memory.rb(cpu.registers.sp) as u16;
    cpu.registers.sp = cpu.registers.sp.wrapping_add(1);
    let hi = cpu.memory.rb(cpu.registers.sp) as u16;
    cpu.registers.sp = cpu.registers.sp.wrapping_add(1);
    (hi << 8) | lo
}

/// `PUSH rr`: 4 M-cycles — opcode fetch, **one internal cycle**, then the two
/// writes. Deferring that internal cycle to the end of the instruction (which
/// is what happens when only bus accesses tick the clock) puts both writes one
/// M-cycle early — `acceptance/push_timing` measures exactly that.
fn push_rr(c: &mut Cpu, value: u16) {
    // That internal cycle is the first `dec sp`: the IDU drives the old SP
    // onto the address bus, which is one of the DMG OAM bug's triggers. The
    // second `dec sp` shares its M-cycle with the high-byte write and folds
    // into it.
    c.memory.idu_tick(c.registers.sp);
    c.memory.internal_tick();
    pushstack(c, value);
}

pub fn pushbc(c: &mut Cpu) {
    let value = ((c.registers.b as u16) << 8) | (c.registers.c as u16);
    push_rr(c, value);
}
pub fn pushde(c: &mut Cpu) {
    let value = ((c.registers.d as u16) << 8) | (c.registers.e as u16);
    push_rr(c, value);
}
pub fn pushhl(c: &mut Cpu) {
    let value = ((c.registers.h as u16) << 8) | (c.registers.l as u16);
    push_rr(c, value);
}
pub fn pushaf(c: &mut Cpu) {
    let value = ((c.registers.a as u16) << 8) | ((c.registers.f & 0xF0) as u16);
    push_rr(c, value);
}
pub fn popbc(c: &mut Cpu) {
    let val = popstack(c);
    c.registers.b = (val >> 8) as u8;
    c.registers.c = (val & 0x00FF) as u8
}
pub fn popde(c: &mut Cpu) {
    let val = popstack(c);
    c.registers.d = (val >> 8) as u8;
    c.registers.e = (val & 0x00FF) as u8;
}
pub fn pophl(c: &mut Cpu) {
    let value = popstack(c);
    c.registers.h = (value >> 8) as u8;
    c.registers.l = (value & 0x00FF) as u8;
}
pub fn popaf(c: &mut Cpu) {
    let v = popstack(c) & 0xFFF0;
    c.registers.a = (v >> 8) as u8;
    c.registers.f = (v & 0x00F0) as u8;
}
pub fn jpnn(c: &mut Cpu) {
    c.registers.pc = c.get_word();
}
pub fn jphl(c: &mut Cpu) {
    let value = ((c.registers.h as u16) << 8) | (c.registers.l as u16);
    c.registers.pc = value;
}
/// `JP cc,nn`: the 16-bit operand is **always** fetched (2 M-cycles) whether or
/// not the jump is taken; only the extra internal cycle is conditional. Skipping
/// the fetch would leave those two M-cycles as untimed slack at the end of the
/// instruction instead of two real bus cycles.
fn jp_cc(c: &mut Cpu, taken: bool) -> u32 {
    let target = c.get_word();
    if taken {
        c.registers.pc = target;
        4
    } else {
        3
    }
}
pub fn jpnznn(c: &mut Cpu) -> u32 {
    let taken = !c.registers.getflag(Z);
    jp_cc(c, taken)
}
pub fn jpznn(c: &mut Cpu) -> u32 {
    let taken = c.registers.getflag(Z);
    jp_cc(c, taken)
}
pub fn jpncnn(c: &mut Cpu) -> u32 {
    let taken = !c.registers.getflag(C);
    jp_cc(c, taken)
}
pub fn jpcnn(c: &mut Cpu) -> u32 {
    let taken = c.registers.getflag(C);
    jp_cc(c, taken)
}
pub fn jrn(c: &mut Cpu) {
    let n = c.get_byte() as i8;
    c.registers.pc = ((c.registers.pc as u32 as i32) + (n as i32)) as u16;
}
/// `JR cc,e`: like `JP cc,nn`, the operand byte is always fetched.
fn jr_cc(c: &mut Cpu, taken: bool) -> u32 {
    let n = c.get_byte() as i8;
    if taken {
        c.registers.pc = ((c.registers.pc as u32 as i32) + (n as i32)) as u16;
        3
    } else {
        2
    }
}
pub fn jrnzn(c: &mut Cpu) -> u32 {
    let taken = !c.registers.getflag(Z);
    jr_cc(c, taken)
}
pub fn jrzn(c: &mut Cpu) -> u32 {
    let taken = c.registers.getflag(Z);
    jr_cc(c, taken)
}

pub fn jrncn(c: &mut Cpu) -> u32 {
    let taken = !c.registers.getflag(C);
    jr_cc(c, taken)
}
pub fn jrcn(c: &mut Cpu) -> u32 {
    let taken = c.registers.getflag(C);
    jr_cc(c, taken)
}

/// `CALL nn`: 6 M-cycles — opcode fetch, the two operand fetches, **one
/// internal cycle**, then the high and low pushes. The operand must be read
/// *before* the return address is pushed (`acceptance/call_timing`,
/// `call_timing2`).
fn call(c: &mut Cpu) {
    let target = c.get_word();
    c.memory.idu_tick(c.registers.sp);
    c.memory.internal_tick();
    let ret_addr = c.registers.pc;
    pushstack(c, ret_addr);
    c.registers.pc = target;
}

/// `CALL cc,nn`: the operand is always fetched (3 M-cycles); only the internal
/// cycle and the two pushes are conditional.
fn call_cc(c: &mut Cpu, taken: bool) -> u32 {
    if taken {
        call(c);
        6
    } else {
        c.get_word();
        3
    }
}

pub fn callnn(c: &mut Cpu) {
    call(c);
}
pub fn callnznn(c: &mut Cpu) -> u32 {
    let taken = !c.registers.getflag(Z);
    call_cc(c, taken)
}
pub fn callznn(c: &mut Cpu) -> u32 {
    let taken = c.registers.getflag(Z);
    call_cc(c, taken)
}
pub fn callncnn(c: &mut Cpu) -> u32 {
    let taken = !c.registers.getflag(C);
    call_cc(c, taken)
}
pub fn callcnn(c: &mut Cpu) -> u32 {
    let taken = c.registers.getflag(C);
    call_cc(c, taken)
}
pub fn ret(c: &mut Cpu) {
    c.registers.pc = popstack(c);
}
pub fn reti(c: &mut Cpu) {
    c.registers.pc = popstack(c);
}

/// `RET cc`: 5 M-cycles when taken — opcode fetch, **one internal cycle for the
/// condition**, the two reads, then a final internal cycle. Only 2 when not
/// taken. Putting that first internal cycle after the reads (which is what
/// happens when it is left as end-of-instruction slack) moves both reads one
/// M-cycle early — `acceptance/ret_cc_timing`.
fn ret_cc(c: &mut Cpu, taken: bool) -> u32 {
    c.memory.internal_tick();
    if taken {
        c.registers.pc = popstack(c);
        5
    } else {
        2
    }
}
pub fn retnz(c: &mut Cpu) -> u32 {
    let taken = !c.registers.getflag(Z);
    ret_cc(c, taken)
}
pub fn retz(c: &mut Cpu) -> u32 {
    let taken = c.registers.getflag(Z);
    ret_cc(c, taken)
}
pub fn retnc(c: &mut Cpu) -> u32 {
    let taken = !c.registers.getflag(C);
    ret_cc(c, taken)
}
pub fn retc(c: &mut Cpu) -> u32 {
    let taken = c.registers.getflag(C);
    ret_cc(c, taken)
}

/// `RST n`: 4 M-cycles, laid out like `PUSH` — opcode fetch, internal, high
/// push, low push (`acceptance/rst_timing`).
pub fn rst(c: &mut Cpu, val: u16) {
    c.memory.idu_tick(c.registers.sp);
    c.memory.internal_tick();
    let ret_addr = c.registers.pc;
    pushstack(c, ret_addr);
    c.registers.pc = val;
}
