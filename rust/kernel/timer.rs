use crate::bindings::{
    timer_list, lock_class_key,
    del_timer_sync, mod_timer,
    init_timer_key,
};
use crate::CStr;

use alloc::boxed::Box;
use core::pin::Pin;
use core::mem::MaybeUninit;
use core::marker::PhantomPinned;
use core::ops::DerefMut;

pub struct TimerBuilder<'a, C> {
    pub name: CStr<'a>,
    pub callback: C,
    pub flags: u32
}

#[macro_export]
macro_rules! timer {
    ($callback:expr) => {
        $crate::timer::TimerBuilder::new(
            // C-macro uses variable name as timer name.
            // In Rust we will be using filename and line instead
            /* name */ $crate::cstr!(concat!(
                core::file!(),
                "_",
                core::line!()
            )),
            $callback
        )
    }
}

/// ```rust
/// let mut timer: Pin<Box<Timer<_>> =
///     TimerBuilder::new(
///         cstr!("HelloTimer"),
///         || println!("Hello from timer!")
///     )
///     .irqsafe()
///     .boxed();
/// timer.modify(jiffies() + 5*HZ)
/// ```
impl<'a, C> TimerBuilder<'a, C> where C: TimerCallback {
    /// Creates new builder with provided name and callback.
    pub fn new(name: CStr<'a>, callback: C) -> Self {
        Self {
            name,
            callback,
            flags: 0
        }
    }

    /// A deferrable timer will work normally when the system is busy, but
    /// will not cause a CPU to come out of idle just to service it; instead,
    /// the timer will be serviced when the CPU eventually wakes up with a
    /// subsequent non-deferrable timer.
    pub fn deferrable(mut self) -> Self {
        self.flags |= crate::bindings::TIMER_DEFERRABLE;
        self
    }

    /// An irqsafe timer is executed with IRQ disabled and it's safe to wait for
    /// the completion of the running instance from IRQ handlers, for example,
    /// by calling del_timer_sync(). FIXME!
    /// 
    /// Note: The irq disabled callback execution is a special case for
    /// workqueue locking issues. It's not meant for executing random crap
    /// with interrupts disabled. Abuse is monitored!
    pub fn irqsafe(mut self) -> Self {
        self.flags |= crate::bindings::TIMER_IRQSAFE;
        self
    }

    /// Creates timer inside of pinned MaybeUninit.
    ///
    /// # Safety
    /// Caller must drop previous timer if there is any.
    pub unsafe fn in_uninit<'b, P>(self, place: &'b mut Pin<P>) -> Pin<&'b mut Timer<'a, C>>
        where
            P: DerefMut<Target = MaybeUninit<Timer<'a, C>>>,
    {
        // SAFETY: We MUST call .initialize() after pinning:
        let (uninit, flags) = Timer::new_uninitialized(self);

        place.set(MaybeUninit::new(uninit));
        let result: Pin<&mut MaybeUninit<_>> = place.as_mut();

        // SAFETY:
        // 1. We just initialized MaybeUninit, so we can dereference pointer.
        // 2. Map function is good in terms of "map_unchecked_mut".
        let result: Pin<&mut _> = result
            .map_unchecked_mut(|x| &mut *x.as_mut_ptr());

        result.initialize(flags)
    }

    /// Creates timer inside of pinned option, dropping and overwriting previous timer if any.
    ///
    /// # Example
    /// ```rust
    /// static mut PINNED_OPTION: Pin<&mut Option<Timer<fn()>>> = unsafe {
    ///     static mut OPTIONAL_TIMER: Option<Timer<fn()>> = None;
    ///     Pin::new_unchecked(&mut OPTIONAL_TIMER)
    /// }
    ///
    /// fn callback() {
    ///     println!("Hello from static timer!")
    /// }
    ///
    /// fn init() {
    ///     let timer: Pin<&mut Timer<_>> = unsafe {
    ///         timer!(callback).in_option(&mut PINNED_OPTION)
    ///     };
    ///     timer.modify(jiffies() + 5*HZ);
    /// }
    /// ```
    pub fn in_option<'b, P>(self, place: &'b mut Pin<P>) -> Pin<&'b mut Timer<'a, C>>
        where P: DerefMut<Target = Option<Timer<'a, C>>>
    {
        unsafe {
            // SAFETY: We MUST call .initialize() after pinning:
            let (uninit, flags) = Timer::new_uninitialized(self);

            place.set(Some(uninit));
            let result: Pin<&mut Option<_>> = place.as_mut();

            // SAFETY:
            // 1. We just initialized Option, so we can unsafely unwrap it.
            //    FIXME: Waiting for [option_result_unwrap_unchecked] stabilized
            //    [option_result_unwrap_unchecked]: https://github.com/rust-lang/rust/issues/63291
            // 2. Map function is good in terms of "map_unchecked_mut".
            let result: Pin<&mut _> = result
                .map_unchecked_mut(|x| x.as_mut().unwrap());

            result.initialize(flags)
        }
    }

    /// Creates timer on heap and returns it.
    ///
    /// Eaisest and safest way to create a timer.
    pub fn boxed(self) -> Pin<Box<Timer<'a, C>>> {
        unsafe {
            // SAFETY: We MUST call .initialize() after pinning:
            let (uninit, flags) = Timer::new_uninitialized(self);

            let mut result: Pin<Box<_>> = Box::pin(uninit);

            result.as_mut().initialize(flags);

            result
        }
    }
}

/// This trait may be extended in future to give more features.
/// So it's not recommended to implement it directly.
/// Better to use closures or any other types that implement FnMut().
pub trait TimerCallback: Send + Sync + Sized {
    fn invoke(&mut self);
}

impl<F> TimerCallback for F where F: Send + Sync + Sized + FnMut() {
    fn invoke(&mut self) {
        (self)()
    }
}

/// Wrapper around then kernel's `timer_list`.
pub struct Timer<'a, C> {
    // Timer is Unpin, since struct timer_list have a pointer to callback function
    // which is stored in the same struct.
    _pinned: PhantomPinned,
    list: MaybeUninit<timer_list>,
    // SAFETY: Do not use this field while there is possibility for callback to be run.
    callback: C,
    name: CStr<'a>,
    key: MaybeUninit<lock_class_key>,
}

// SAFETY: both timer_list and key are Send+Sync in Rust terminology.
unsafe impl<'a, C> Send for Timer<'a, C> where C: Send {}
unsafe impl<'a, C> Sync for Timer<'a, C> where C: Sync {}

impl<'a, C> Timer<'a, C> where C: TimerCallback {
    unsafe extern "C" fn wrapper(list: *mut timer_list) {
        let container: *const Self = crate::container_of!(list, Self, list);
        let offset_in_bytes = crate::offset_of!(Self, callback);
        let callback: *const C = (container as *const u8).offset(offset_in_bytes) as *const _;

        // SAFETY:
        // Timer is owned somewhere else and thus be modified unexpectedly.
        // Here we are creating reference to the field bypassing Rust's borrowing rules,
        // so theoretically callback can be dropped right after dereferencing pointer.
        // But is safe for a reasons:
        // 1. We do not allow changing callback while callback can be called by kernel.
        // 2. Timer won't finish dropping until callback can be called.
        // 3. callback field is not accessed anywhere else at this moment, so it's safe to create
        //    an mutable reference mut.
        let callback = &mut *(callback as *mut C);
        callback.invoke();
    }

    /// Calls init_timer_key and initializes all required fields.
    ///
    /// # Safety
    /// Caller must ensure that place contains correct struct, with timer and key uninitialized.
    /// This function is guaranteed to be safe when calling right after new_uninitialized
    unsafe fn initialize(self: Pin<&mut Self>, flags: u32) -> Pin<&mut Self> {
        // FIXME: It's better to avoid using as_ptr() and then casting to *mut T
        //        But Pin protects us from accessing &mut Self directly.
        init_timer_key(
            /* timer */ self.list.as_ptr() as *mut _,
            /* func  */ Some(Self::wrapper),
            /* flags */ flags,
            /* name  */ self.name.as_ptr() as *const i8,
            /* key   */ self.key.as_ptr() as *mut _,
        );
        self
    }

    /// # Safety
    /// This function returns Timer without initializing it's fields.
    /// Caller MUST call .initialize() before using timer.
    unsafe fn new_uninitialized(args: TimerBuilder<'a, C>) -> (Self, u32) {
        let res = Self {
            _pinned: PhantomPinned::default(),
            list: MaybeUninit::uninit(),
            name: args.name,
            key: MaybeUninit::uninit(),
            callback: args.callback
        };
        (res, args.flags)
    }

    /// Returns was the timer active.
    /// ie. modifying inactive timer will return false.
    pub fn modify(self: Pin<&mut Self>, expires: u64) -> bool {
        let res = unsafe {
            // SAFETY: We won't move any data
            let this = self.get_unchecked_mut();
            // SAFETY: self.list is initialized
            mod_timer(this.list.as_mut_ptr(), expires)
        };
        res != 0
    }
}

impl<F> Drop for Timer<'_, F> {
    fn drop(&mut self) {
        unsafe {
            // SAFETY: self.list is initialized.
            let timer_list = self.list.as_mut_ptr();

            // SAFETY: We must guarantee that callback won't be called after Timer is dropped,
            //         since callback will be dropped too. So let's call del_timer_sync.
            del_timer_sync(timer_list);
            core::ptr::drop_in_place(timer_list);

            // SAFETY: self.key is initialized.
            core::ptr::drop_in_place(self.key.as_mut_ptr());
        }
    }
}
