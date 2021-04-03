// SPDX-License-Identifier: GPL-2.0

//! Rust example module

#![no_std]
#![feature(allocator_api, global_asm)]
#![feature(const_pin, const_mut_refs)]

use alloc::boxed::Box;
use core::pin::Pin;
use kernel::prelude::*;
use kernel::timer::{Timer, TimerCallback};
use kernel::time::{jiffies, HZ};

module! {
    type: RustTimerExample,
    name: b"rust_timer_example",
    author: b"Rust for Linux Contributors",
    description: b"An example of using timers in Rust",
    license: b"GPL v2",
    params: {},
}

struct MyHandler {
    message: String
}

impl TimerCallback for MyHandler {
    fn invoke(&mut self) {
        println!("Hello from static timer. {}", self.message)
    }
}

fn static_callback() {
    println!("Hello from static_callback");
}

struct RustTimerExample {
    boxed_timer: Pin<Box<Timer<'static, MyHandler>>>,
    static_timer: Pin<&'static mut Timer<'static, fn()>>
}

impl KernelModule for RustTimerExample {
    fn init() -> KernelResult<Self> {
        let message = String::from("Message on heap!");
        let handler = MyHandler { message };
        let mut boxed_timer = kernel::timer!(handler).boxed();

        static mut TIMER: Pin<&'static mut Option<Timer<fn()>>> = unsafe {
            static mut STATIC_PLACE_FOR_TIMER: Option<Timer<fn()>> = None;
            // SAFETY: Pinning &'static mut is safe:
            Pin::new_unchecked(&mut STATIC_PLACE_FOR_TIMER)
        };
        let builder = kernel::timer!(static_callback as _);
        let mut static_timer = unsafe {
            // It's safe to use static mut here, because only single instance of kernel module may
            // exist at any givern time in kernel.
            builder.in_option(&mut TIMER)
        };

        boxed_timer.as_mut().modify(jiffies().wrapping_add(3*HZ));
        static_timer.as_mut().modify(jiffies().wrapping_add(5*HZ));
        println!("Timers example initialized!");

        Ok(RustTimerExample {
            boxed_timer,
            static_timer
        })
    }
}

