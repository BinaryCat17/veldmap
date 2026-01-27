use std::os::raw::{c_void, c_int, c_ulong};
use crate::api::VeldMap;
use raw_window_handle::{
    RawWindowHandle, RawDisplayHandle, 
    HasWindowHandle, HasDisplayHandle,
    HandleError, WindowHandle, DisplayHandle,
    XlibWindowHandle, XlibDisplayHandle,
    WaylandWindowHandle, WaylandDisplayHandle,
    Win32WindowHandle, AppKitWindowHandle, AppKitDisplayHandle
};

pub struct VeldMapEngine {
    pub(crate) inner: VeldMap,
}

/// Перечисление поддерживаемых оконных систем.
/// Вызывающая сторона должна указать, какой тип дескрипторов она передает.
#[repr(C)]
pub enum WindowBackend {
    X11 = 0,
    Wayland = 1,
    Win32 = 2,
    Cocoa = 3, // macOS
}

/// Универсальная структура для передачи дескрипторов окна.
/// ptr1 и ptr2 зависят от выбранного backend.
#[repr(C)]
pub struct NativeWindowDesc {
    pub backend: WindowBackend,
    pub ptr1: *mut c_void, // X11: Display*, Win32: HTSANCE, Wayland: Display*
    pub ptr2: *mut c_void, // X11: Window,   Win32: HWND,    Wayland: Surface*
}

/// Внутренняя обертка, превращающая сырые указатели в Rust-типы raw-window-handle.
struct FfiWindowWrapper {
    window: RawWindowHandle,
    display: RawDisplayHandle,
}

// Указываем компилятору, что эту структуру можно безопасно передавать между потоками.
// ВАЖНО: Вызывающая сторона (C/C++/Python) обязана гарантировать, 
// что указатели на окна валидны в течение всего времени жизни движка.
unsafe impl Send for FfiWindowWrapper {}
unsafe impl Sync for FfiWindowWrapper {}

impl HasWindowHandle for FfiWindowWrapper {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        // Safety: мы создаем валидный handle из данных, предоставленных пользователем
        unsafe { Ok(WindowHandle::borrow_raw(self.window)) }
    }
}

impl HasDisplayHandle for FfiWindowWrapper {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        // Safety: мы создаем валидный handle из данных, предоставленных пользователем
        unsafe { Ok(DisplayHandle::borrow_raw(self.display)) }
    }
}

#[no_mangle]
pub extern "C" fn veldmap_create_engine(
    desc: NativeWindowDesc,
    width: u32,
    height: u32,
) -> *mut VeldMapEngine {
    
    let (window_handle, display_handle) = match desc.backend {
        WindowBackend::X11 => {
            // ptr1 = Display*, ptr2 = Window (unsigned long)
            let w = XlibWindowHandle::new(desc.ptr2 as usize as c_ulong);
            let d = XlibDisplayHandle::new(std::ptr::NonNull::new(desc.ptr1), 0);
            (RawWindowHandle::Xlib(w), RawDisplayHandle::Xlib(d))
        },
        WindowBackend::Wayland => {
             // ptr1 = wl_display*, ptr2 = wl_surface*
            let w = WaylandWindowHandle::new(std::ptr::NonNull::new(desc.ptr2).unwrap());
            let d = WaylandDisplayHandle::new(std::ptr::NonNull::new(desc.ptr1).unwrap());
            (RawWindowHandle::Wayland(w), RawDisplayHandle::Wayland(d))
        },
        WindowBackend::Win32 => {
            // ptr1 = HINSTANCE, ptr2 = HWND
            // raw-window-handle win32 requires NonNull for Hinstance
            let mut w = Win32WindowHandle::new(std::num::NonZeroIsize::new(desc.ptr2 as isize).unwrap());
            let d = raw_window_handle::WindowsDisplayHandle::new(); 
            // HINSTANCE обычно не обязателен для создания DisplayHandle в wgpu, но может пригодиться
            w.hinstance = std::num::NonZeroIsize::new(desc.ptr1 as isize);
            (RawWindowHandle::Win32(w), RawDisplayHandle::Windows(d))
        },
        WindowBackend::Cocoa => {
            // macOS (Cocoa)
            // ptr1 = NSView*
            let w = AppKitWindowHandle::new(std::ptr::NonNull::new(desc.ptr1).unwrap());
            let d = AppKitDisplayHandle::new();
            (RawWindowHandle::AppKit(w), RawDisplayHandle::AppKit(d))
        },
    };

    let window_wrapper = FfiWindowWrapper {
        window: window_handle,
        display: display_handle,
    };

    let mut veldmap = pollster::block_on(VeldMap::new(window_wrapper));
    veldmap.resize(width, height);
    
    let engine = Box::new(VeldMapEngine { inner: veldmap });
    Box::into_raw(engine)
}

#[no_mangle]
pub extern "C" fn veldmap_render(engine_ptr: *mut VeldMapEngine) -> c_int {
    if engine_ptr.is_null() { return -1; }
    let engine = unsafe { &mut *engine_ptr };
    
    engine.inner.update();
    match engine.inner.render() {
        Ok(_) => 0,
        Err(_) => 1,
    }
}

#[no_mangle]
pub extern "C" fn veldmap_resize(engine_ptr: *mut VeldMapEngine, width: u32, height: u32) {
    if engine_ptr.is_null() { return; }
    let engine = unsafe { &mut *engine_ptr };
    engine.inner.resize(width, height);
}

#[no_mangle]
pub extern "C" fn veldmap_destroy_engine(engine_ptr: *mut VeldMapEngine) {
    if !engine_ptr.is_null() {
        unsafe { drop(Box::from_raw(engine_ptr)); }
    }
}

#[no_mangle]
pub extern "C" fn veldmap_camera_zoom(engine_ptr: *mut VeldMapEngine, delta: f64) {
    if engine_ptr.is_null() { return; }
    let engine = unsafe { &mut *engine_ptr };
    engine.inner.camera_zoom(delta);
}

#[no_mangle]
pub extern "C" fn veldmap_camera_move(engine_ptr: *mut VeldMapEngine, dx: f64, dy: f64) {
    if engine_ptr.is_null() { return; }
    let engine = unsafe { &mut *engine_ptr };
    engine.inner.camera_move(dx, dy);
}