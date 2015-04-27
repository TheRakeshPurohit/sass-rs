#![allow(dead_code)]
use sass_sys;
use std::ffi;
use std::mem;
use sass_value::SassValue;
use sass_function::SassFunction;
use sass_context::SassOptions;
use std::sync::mpsc::{SyncSender,Receiver,sync_channel};
use std::sync::{RwLock,Arc};



/// Message being sent from C to the library.
struct CustomFunctionCall {
    slot: usize,
    argument: SassValue,
    reply: SyncSender<SassValue>
}


/// Struct used as the `cookie` to the C dispatch function.
struct DispatchSlot {
    sender: SyncSender<CustomFunctionCall>,
    slot: usize
}

impl DispatchSlot {
    pub fn send(&self, sass_value:SassValue) -> SassValue {
        let (tx,rx) = sync_channel::<SassValue>(0);
        let message = CustomFunctionCall {
            slot: self.slot,
            argument: sass_value,
            reply: tx
        };
        match self.sender.send(message) {
            Ok(_) => {
                match rx.recv() {
                    Ok(value) => value,
                    Err(_) => SassValue::sass_error("send error")
                }
            },
            Err(_) => {
                SassValue::sass_error("send error")
            }
        }
    }
}


/// Holds the data structures needed to dispatch calls from libsass
/// back in the Rust code
pub struct Dispatcher {
    providers: Vec<Box<SassFunction>>,
    receiver: Receiver<CustomFunctionCall>,
    dispatch_slots: Vec<Box<DispatchSlot>>,
    sass_options: Arc<RwLock<SassOptions>>
}


impl Dispatcher {

    pub fn build(registry:Vec<(&'static str,Box<SassFunction>)>, sass_options:Arc<RwLock<SassOptions>>) -> Dispatcher {
        let (tx,rx) = sync_channel::<CustomFunctionCall>(0);
        let mut _providers = Vec::new();
        let mut callbacks = Vec::new();
        let mut _slots = Vec::new();
        for (index,one) in registry.into_iter().enumerate() {
            let slot = Box::new(DispatchSlot {sender:tx.clone(),slot:index});

            callbacks.push(Dispatcher::create_callback(one.0,&slot));
            _providers.push(one.1);
            _slots.push(slot)
        }
        let _ = sass_options.write().map(|mut o| {
            o.set_sass_functions(callbacks)
        });
        Dispatcher {
            providers: _providers,
            receiver: rx,
            dispatch_slots: _slots,
            sass_options: sass_options
        }
    }


    /// The dispatch function, this should be called until it returns an error.
    /// The caller should probably use a different thread.
    pub fn dispatch(&self) -> Result<(),String> {
        match self.receiver.recv() {
            Ok(message) => {
                let _fn:&Box<SassFunction>  = &self.providers[message.slot];
                let out = _fn.custom(&message.argument);
                message.reply.send(out).map_err(|_| "dispatch reply error".to_string())
            },
            Err(_) => Err("dispatch recv error".to_string())
        }
    }


    fn create_callback( signature:&str,_fn:&Box<DispatchSlot>) -> sass_sys::Sass_C_Function_Callback {
        // NOTE: this generates a memory leak, store in Dispatcher.
        let boxed = Box::new(ffi::CString::new(signature).unwrap());

        unsafe {
            // move the value outside the rust memory management model
            let c_sig: *const ffi::CString = mem::transmute(boxed);

            // use
            sass_sys::sass_make_function((&*c_sig).as_ptr(), Some(dispatch), mem::transmute(_fn))
        }
    }

}

impl Drop for Dispatcher {
    fn drop(&mut self) {
        let _ = self.sass_options.write().map(|mut o| o.set_sass_functions(Vec::new()));
    }
}


/// Dispatcher function called from libsass (C interface).
/// The cookie argument is setup in SassFunctionCallback::from_sig_fn.
/// Note that the SassFunctionCallback is not used directly in the dispatch.
extern "C" fn dispatch(arg1: *const sass_sys::Union_Sass_Value,
                       cookie: *mut ::libc::c_void) -> *mut sass_sys::Union_Sass_Value {
    let dispatch_slot:Box<DispatchSlot> = unsafe {mem::transmute(cookie)};

    match dispatch_slot.send(SassValue::from_raw(arg1)).as_raw() {
        Some(raw) => raw,
        None => SassValue::sass_error("bad call").as_raw().unwrap()
    }

}
