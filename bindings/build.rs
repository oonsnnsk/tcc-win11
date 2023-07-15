// fn main() {
//     windows::build!(
//         Windows::Win32::UI::WindowsAndMessaging::{
//             WNDCLASSW, WNDPROC, WNDCLASS_STYLES,
//             WS_OVERLAPPEDWINDOW, CW_USEDEFAULT, WINDOW_EX_STYLE,
//             SHOW_WINDOW_CMD, RegisterClassW, CreateWindowExW,
//             ShowWindow, DefWindowProcW, PostQuitMessage, 
//             GetMessageW, MSG, TranslateMessage, DispatchMessageW, WM_DESTROY, WM_SIZE, 
//         },
//         Windows::Win32::UI::MenusAndResources::{HICON, HCURSOR, HMENU},
//         Windows::Win32::Graphics::Gdi::HBRUSH,
//         Windows::Win32::System::SystemServices::{
//             PWSTR, HINSTANCE, LRESULT, GetModuleHandleW,
//         },
//     )
// }

fn main() {
    let _ = embed_manifest::embed_manifest_file("bindings/app.manifest");
}
