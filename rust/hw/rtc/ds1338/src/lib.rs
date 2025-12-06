// Copyright 2025 HUST OpenAtom Open Source Club.
// SPDX-License-Identifier: GPL-2.0-or-later

use bql::BqlRefCell;
use hwcore::{
    DeviceImpl, I2CEvent, I2CResult, I2CSlave, I2CSlaveClass, I2CSlaveImpl, ResetType,
    ResettablePhasesImpl,
};
use migration::{
    impl_vmstate_struct, vmstate_fields, VMStateDescription, VMStateDescriptionBuilder,
};
use qom::{qom_isa, ObjectImpl, ParentField};
use std::mem::MaybeUninit;
use system::bindings::{qemu_get_timedate, qemu_timedate_diff, tm};

pub const TYPE_DS1338: &::std::ffi::CStr = c"ds1338";
const NVRAM_SIZE: usize = 64;
const HOURS_12: u8 = 0x40;
const HOURS_PM: u8 = 0x20;
const CTRL_OSF: u8 = 0x20;

fn to_bcd(x: u8) -> u8 {
    (x / 10) * 16 + (x % 10)
}

fn from_bcd(x: u8) -> u8 {
    (x / 16) * 10 + (x % 16)
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct DS1338Inner {
    pub offset: i64,
    pub wday_offset: u8,
    pub nvram: [u8; NVRAM_SIZE],
    pub ptr: i32,
    pub addr_byte: bool,
}

impl Default for DS1338Inner {
    fn default() -> Self {
        Self {
            offset: 0,
            wday_offset: 0,
            nvram: [0; NVRAM_SIZE],
            ptr: 0,
            addr_byte: false,
        }
    }
}

impl_vmstate_struct!(
    DS1338Inner,
    VMStateDescriptionBuilder::<DS1338Inner>::new()
        .name(c"ds1338/inner")
        .version_id(2)
        .minimum_version_id(1)
        .fields(vmstate_fields! {
            migration::vmstate_of!(DS1338Inner, offset),
            migration::vmstate_of!(DS1338Inner, wday_offset),
            migration::vmstate_of!(DS1338Inner, nvram),
            migration::vmstate_of!(DS1338Inner, ptr),
            migration::vmstate_of!(DS1338Inner, addr_byte),
        })
        .build()
);

#[repr(C)]
#[derive(qom::Object, hwcore::Device)]
pub struct DS1338State {
    pub parent_obj: ParentField<I2CSlave>,
    pub inner: BqlRefCell<DS1338Inner>,
}

impl DS1338Inner {
    pub fn capture_current_time(&mut self) {
        unsafe {
            let mut now = MaybeUninit::<tm>::uninit();
            qemu_get_timedate(now.as_mut_ptr(), self.offset);
            let now = now.assume_init();

            self.nvram[0] = to_bcd(now.tm_sec as u8);
            self.nvram[1] = to_bcd(now.tm_min as u8);

            if self.nvram[2] & HOURS_12 != 0 {
                let mut tmp = now.tm_hour;
                if tmp % 12 == 0 {
                    tmp += 12;
                }
                if tmp <= 12 {
                    self.nvram[2] = HOURS_12 | to_bcd(tmp as u8);
                } else {
                    self.nvram[2] = HOURS_12 | HOURS_PM | to_bcd((tmp - 12) as u8);
                }
            } else {
                self.nvram[2] = to_bcd(now.tm_hour as u8);
            }

            self.nvram[3] = ((now.tm_wday + self.wday_offset as i32) % 7 + 1) as u8;
            self.nvram[4] = to_bcd(now.tm_mday as u8);
            self.nvram[5] = to_bcd((now.tm_mon + 1) as u8);
            self.nvram[6] = to_bcd((now.tm_year - 100) as u8);
        }
    }

    pub fn inc_regptr(&mut self) {
        self.ptr = (self.ptr + 1) & (NVRAM_SIZE as i32 - 1);
        if self.ptr == 0 {
            self.capture_current_time();
        }
    }

    pub fn get_reg(&self) -> u8 {
        self.nvram[self.ptr as usize]
    }

    pub fn set_ptr(&mut self, data: u8) {
        self.ptr = (data & (NVRAM_SIZE as u8 - 1)) as i32;
    }

    pub fn write_time_register(&mut self, data: u8) {
        unsafe {
            let mut now = MaybeUninit::<tm>::uninit();
            qemu_get_timedate(now.as_mut_ptr(), self.offset);
            let mut now = now.assume_init();

            match self.ptr {
                0 => {
                    now.tm_sec = from_bcd(data & 0x7f) as i32;
                }
                1 => {
                    now.tm_min = from_bcd(data & 0x7f) as i32;
                }
                2 => {
                    if data & HOURS_12 != 0 {
                        let mut tmp = from_bcd(data & (HOURS_PM - 1)) as i32;
                        if data & HOURS_PM != 0 {
                            tmp += 12;
                        }
                        if tmp % 12 == 0 {
                            tmp -= 12;
                        }
                        now.tm_hour = tmp;
                    } else {
                        now.tm_hour = from_bcd(data & (HOURS_12 - 1)) as i32;
                    }
                }
                3 => {
                    let user_wday = ((data & 7) as i32) - 1;
                    self.wday_offset = ((user_wday - now.tm_wday + 7) % 7) as u8;
                }
                4 => {
                    now.tm_mday = from_bcd(data & 0x3f) as i32;
                }
                5 => {
                    now.tm_mon = from_bcd(data & 0x1f) as i32 - 1;
                }
                6 => {
                    now.tm_year = from_bcd(data) as i32 + 100;
                }
                _ => {}
            }

            self.offset = qemu_timedate_diff(&mut now as *mut tm);
        }
    }

    pub fn write_control_register(&mut self, data: u8) {
        let mut data = data & 0xB3;
        data = (data & !CTRL_OSF) | (data & self.nvram[self.ptr as usize] & CTRL_OSF);

        self.nvram[self.ptr as usize] = data;
    }

    pub fn write_nvram(&mut self, data: u8) {
        self.nvram[self.ptr as usize] = data;
    }
}

qom_isa!(DS1338State: I2CSlave, hwcore::DeviceState, qom::Object);

unsafe impl qom::ObjectType for DS1338State {
    type Class = I2CSlaveClass;
    const TYPE_NAME: &'static std::ffi::CStr = TYPE_DS1338;
}

impl ObjectImpl for DS1338State {
    type ParentType = I2CSlave;
    const CLASS_INIT: fn(&mut Self::Class) = Self::Class::class_init::<Self>;
}

impl DeviceImpl for DS1338State {
    const VMSTATE: Option<migration::VMStateDescription<Self>> = Some(VMSTATE_DS1338);
}

impl I2CSlaveImpl for DS1338State {
    const RECV: Option<fn(&Self) -> u8> = Some(Self::recv);
    const SEND: Option<fn(&Self, data: u8) -> I2CResult> = Some(Self::send);
    const EVENT: Option<fn(&Self, event: I2CEvent) -> I2CEvent> = Some(Self::event);
}

impl DS1338State {
    fn recv(&self) -> u8 {
        let mut inner = self.inner.borrow_mut();
        let res = inner.get_reg();
        inner.inc_regptr();
        res
    }

    fn send(&self, data: u8) -> I2CResult {
        let mut inner = self.inner.borrow_mut();

        if inner.addr_byte {
            inner.set_ptr(data);
            inner.addr_byte = false;
            return I2CResult::ACK;
        }

        if inner.ptr < 7 {
            inner.write_time_register(data);
        } else if inner.ptr == 7 {
            inner.write_control_register(data);
        } else {
            inner.write_nvram(data);
        }

        inner.inc_regptr();
        I2CResult::ACK
    }

    fn event(&self, event: I2CEvent) -> I2CEvent {
        let mut inner = self.inner.borrow_mut();

        match event {
            I2CEvent::START_RECV => {
                inner.capture_current_time();
            }
            I2CEvent::START_SEND => {
                inner.addr_byte = true;
            }
            _ => {}
        }

        I2CEvent::START_RECV
    }

    fn reset_hold(&self, _reset_type: ResetType) {
        let mut inner = self.inner.borrow_mut();
        inner.offset = 0;
        inner.wday_offset = 0;
        inner.ptr = 0;
        inner.addr_byte = false;
        inner.nvram.fill(0);
    }
}

impl ResettablePhasesImpl for DS1338State {
    const HOLD: Option<fn(&Self, ResetType)> = Some(Self::reset_hold);
}

pub const VMSTATE_DS1338: VMStateDescription<DS1338State> =
    VMStateDescriptionBuilder::<DS1338State>::new()
        .name(c"ds1338")
        .version_id(2)
        .minimum_version_id(1)
        .fields(vmstate_fields! {
            migration::vmstate_of!(DS1338State, inner),
        })
        .build();
