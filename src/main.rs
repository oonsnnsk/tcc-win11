#![windows_subsystem = "windows"]

use std::{
    *,
    thread::*,
    collections::HashMap,
    sync::Mutex,
    sync::mpsc::*,
    borrow::*,
};
use windows::{
    core::*,
    imp::WaitForSingleObject,
    Win32,
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::LibraryLoader::GetModuleHandleW,
        System::Threading::*,
        UI::Input::KeyboardAndMouse::*,
        UI::WindowsAndMessaging::*,
        UI::HiDpi::*,
        Devices::Display::*,
        System::Performance::*,
        System::ProcessStatus::*,
    },
};
use once_cell::sync::Lazy;
use chrono::*;
use chrono_tz::*;
use regex::*;
use anyhow::*;
use wmi::*;
use winreg::enums::*;
use winreg::RegKey;
use winput::*;

fn convert_utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn convert_utf16_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn is_lock_screen() -> bool {
    unsafe {
        let mut hwnd = GetForegroundWindow();
        if hwnd.0 == 0 {
            // ?
            return true;
        }
        loop {
            let mut classname:Vec<u8> = vec![0; 100];
            GetClassNameA(hwnd, &mut classname);
            let a = String::from_utf8(classname).unwrap();
            let b = a.trim_end_matches(char::from(0));
            if b == "Windows.UI.Core.CoreWindow" {
                let mut pid: u32 = 0;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                let hnd = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ , false, pid).unwrap();
                let mut modulename:Vec<u8> = vec![0; 100];
                GetModuleBaseNameA(hnd, None, &mut modulename);
                let a = String::from_utf8(modulename).unwrap();
                let b = a.trim_end_matches(char::from(0));            
                CloseHandle(hnd);
                if b.to_lowercase() == "lockapp.exe" {
                    return true;
                }
            } else if b == "LockScreenControllerProxyWindow" {
                // ?
                return true;
            }            
            hwnd = GetWindow(hwnd, GW_HWNDNEXT);
            if hwnd.0 == 0 {
                return false;
            }
        }        
    }
}

fn error_messagebox(msg1: &str, msg2: &str) {
    unsafe {
        let msg = "[Error] ".to_string() + msg1 + "\n\n" + msg2;
        let text = convert_utf16_null(&msg);
        let caption  = convert_utf16_null("tcc-win11");
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            PCWSTR(caption.as_ptr()),
            MB_OK
        );
    }
}

fn get_colorref(rgb: &str) -> COLORREF {
    let r: u32 = u32::from_str_radix(&rgb[0..2], 16).unwrap();
    let g: u32 = u32::from_str_radix(&rgb[2..4], 16).unwrap();
    let b: u32 = u32::from_str_radix(&rgb[4..6], 16).unwrap();
    COLORREF((b << 16) + (g << 8) + r)
}

static GLOBAL_HFONT_HM: Lazy<Mutex<HashMap<String, HFONT>>> = Lazy::new(|| Mutex::new(HashMap::new()));
fn create_global_hfont(key: String, name: String, size: i32, bold: i32, italic: i32) {
    unsafe {
        if !GLOBAL_HFONT_HM.lock().unwrap().contains_key(&key) {
            let font_name = convert_utf16_null(&name);
            let font_weight = match bold {
                1 => FW_BOLD.0,
                _ => FW_REGULAR.0
            };
            let font_italic = match italic {
                1 => 1,
                _ => 0
            };
            let hfont = CreateFontW(
                size,
                0,
                0,
                0,
                font_weight as i32,
                font_italic,
                0,
                0,
                DEFAULT_CHARSET.0 as u32,
                OUT_DEFAULT_PRECIS.0 as u32,
                CLIP_DEFAULT_PRECIS.0 as u32,
                ANTIALIASED_QUALITY.0 as u32,
                FIXED_PITCH.0 as u32,
                PCWSTR(font_name.as_ptr())
            );
            GLOBAL_HFONT_HM.lock().unwrap().insert(key, hfont);
        }
    }
}

static GLOBAL_HPEN_HM: Lazy<Mutex<HashMap<String, HPEN>>> = Lazy::new(|| Mutex::new(HashMap::new()));
fn create_global_hpen(key: &str, color: &str) {
    unsafe {
        if !GLOBAL_HPEN_HM.lock().unwrap().contains_key(key) {
            let hpen = CreatePen(PS_SOLID , 0, get_colorref(color));
            GLOBAL_HPEN_HM.lock().unwrap().insert(key.to_string(), hpen);
        }
    }
}

#[derive(Default, Debug, Clone)]
struct TccPanelInfo {
    global_tcc_display_id: String,
    panel_id: String,
    show_desktop_button: i32,
    show_desktop_button_visible: bool,
}

static GLOBAL_TCC_PANEL_INFO: Lazy<Mutex<HashMap<isize, TccPanelInfo>>> = Lazy::new(|| Mutex::new(HashMap::new()));

fn set_global_tcc_panel_info(k: isize, v: TccPanelInfo) {
    GLOBAL_TCC_PANEL_INFO.lock().unwrap().insert(k, v);
}

#[derive(Default, Debug)]
struct TccTaskbarChecker {
    display_rect: RECT,
    taskbar_rect: RECT,
    taskbar_adjust_left: i32,
    taskbar_adjust_right: i32,
    taskbar_tray_hwnd: HWND,
    taskbar_content_hwnd: HWND,
    taskbar_topmost: bool,
    taskbar_visible: i32,
    display_id: String,
    panel_id_vec: Vec<(String, HWND)>,
    panel_background_init: bool
}

#[derive(Default, Debug, Clone)]
struct TccDisplay {
    id: String,
    zoom: f64,
    device_name: String,
    device_path: String,
    display_rect: RECT,
    taskbar_tray_hwnd: HWND,
    taskbar_content_hwnd: HWND,
    taskbar_rect: RECT,
    taskbar_adjust_left: i32,
    taskbar_adjust_right: i32,
    panels: HashMap<String, TccPanel>,
}

#[derive(Default, Debug, Clone)]
struct TccPanel {
    id: String,
    hwnd: HWND,
    position: i32,  // 0:left 1:center 2:right
    width: i32,
    left: i32,
    show_desktop_button_position: i32,
    labels: Vec<TccLabel>,
    background_init: bool,
    background_dc: CreatedHDC,
    background_bm: HBITMAP,
}

#[derive(Default, Debug, Clone)]
struct TccLabel {
    id: String,
    timezone: String,
    format: String,
    left: i32,
    top: i32,
    font_color: String,
    font_name: String,
    font_size: i32,
    font_bold: i32,
    font_italic: i32,
}
impl TccLabel {
    fn draw(&mut self, hdc: CreatedHDC, now_utc: DateTime<Utc>) {
        unsafe {
            SetBkMode(hdc ,TRANSPARENT);
        }
        unsafe {
            SetTextColor(hdc, get_colorref(&self.font_color));
        }
        create_global_hfont(self.id.to_string(), self.font_name.to_string(), self.font_size, self.font_bold, self.font_italic);
        let hfont_hm = GLOBAL_HFONT_HM.lock().unwrap();
        let hfont = hfont_hm.get(&self.id).unwrap();
        unsafe {
            SelectObject(hdc, *hfont);
        }

        // timezone
        let tz: Tz = self.timezone.parse().unwrap();
        let now = now_utc.with_timezone(&tz);

        // custom format
        let global_tcc_custom_format_hm = GLOBAL_TCC_CUSTOM_FORMAT.lock().unwrap();
        let re = Regex::new(r"\{(.*?)\}").unwrap();
        let re_gpu = Regex::new(r"(-|_|0){0,1}gpu([0-9]+)").unwrap();

        let replace_result = re.replace_all(
            &self.format,
            |caps: &Captures| {
                let custom_format_id = &caps[1];
                return match custom_format_id {
                    "cpu" => {
                        let pref_hm = GLOBAL_PREF.lock().unwrap();
                        let pref_value = pref_hm.get("cpu").unwrap_or(&0);
                        format!("{pref_value:>3}")
                    }
                    "_cpu" => {
                        let pref_hm = GLOBAL_PREF.lock().unwrap();
                        let pref_value = pref_hm.get("cpu").unwrap_or(&0);
                        format!("{pref_value:>3}")
                    }
                    "0cpu" => {
                        let pref_hm = GLOBAL_PREF.lock().unwrap();
                        let pref_value = pref_hm.get("cpu").unwrap_or(&0);
                        format!("{pref_value:0>3}")
                    }
                    "-cpu" => {
                        let pref_hm = GLOBAL_PREF.lock().unwrap();
                        let pref_value = pref_hm.get("cpu").unwrap_or(&0);
                        pref_value.to_string()
                    }
                    "mem" => {
                        let pref_hm = GLOBAL_PREF.lock().unwrap();
                        let pref_value = pref_hm.get("mem").unwrap_or(&0);
                        format!("{pref_value:>3}")
                    }
                    "_mem" => {
                        let pref_hm = GLOBAL_PREF.lock().unwrap();
                        let pref_value = pref_hm.get("mem").unwrap_or(&0);
                        format!("{pref_value:>3}")
                    }
                    "0mem" => {
                        let pref_hm = GLOBAL_PREF.lock().unwrap();
                        let pref_value = pref_hm.get("mem").unwrap_or(&0);
                        format!("{pref_value:0>3}")
                    }
                    "-mem" => {
                        let pref_hm = GLOBAL_PREF.lock().unwrap();
                        let pref_value = pref_hm.get("mem").unwrap_or(&0);
                        pref_value.to_string()
                    }
                    _ => {
                        match re_gpu.captures(custom_format_id) {
                            Some(caps) => {
                                let pref_hm = GLOBAL_PREF.lock().unwrap();
                                match pref_hm.get(&format!("gpu{}", &caps[2])) {
                                    Some(pref_value) => {
                                        let x = match caps.get(1) {
                                            Some(x) => { x.as_str() }
                                            None => { "" }
                                        };
                                        match x {
                                            "" => {
                                                return format!("{pref_value:>3}");
                                            }
                                            "_" => {
                                                return format!("{pref_value:>3}");
                                            }
                                            "0" => {
                                                return format!("{pref_value:0>3}");
                                            }
                                            "-" => {
                                                return pref_value.to_string();
                                            }
                                            _ => {}
                                        }
                                    },
                                    None => {},
                                }
                            }
                            None => {},
                        }
                        match global_tcc_custom_format_hm.get(custom_format_id) {
                            None => {
                                "".to_string()
                            }
                            Some(tcc_custom_format) => {
                                let replace_source = now.format(&tcc_custom_format.value).to_string();
                                if tcc_custom_format.items.contains_key(&replace_source) {
                                    tcc_custom_format.items[&replace_source].to_string()
                                } else if tcc_custom_format.items.contains_key("_") {
                                    tcc_custom_format.items["_"].to_string()
                                } else {
                                    "".to_string()
                                }
                            }
                        }
                    }
                };
            }
        );
        let replaced_format = match replace_result {
            Cow::Borrowed(s) => s.to_owned(),
            Cow::Owned(s) => s,
        };

        // format
        // https://docs.rs/chrono/latest/chrono/format/strftime/index.html
        let text = now.format(&replaced_format).to_string();

        unsafe {
            TextOutW(
                hdc,
                self.left,
                self.top,
                &convert_utf16(&text)
            );
        }
    }
}

static GLOBAL_TCC_DISPLAY: Lazy<Mutex<HashMap<String, TccDisplay>>> = Lazy::new(|| Mutex::new(HashMap::new()));

fn set_global_tcc_display(k: String, v: TccDisplay) {
    GLOBAL_TCC_DISPLAY.lock().unwrap().insert(k, v);
}

#[derive(Debug, Default)]
struct TccCustomFormat {
    spec: String,
    value: String,
    items: HashMap<String, String>
}

static GLOBAL_TCC_CUSTOM_FORMAT: Lazy<Mutex<HashMap<String, TccCustomFormat>>> = Lazy::new(|| Mutex::new(HashMap::new()));


static GLOBAL_HWND: Lazy<Mutex<HashMap<String, HWND>>> = Lazy::new(|| Mutex::new(HashMap::new()));

fn set_global_hwnd(k: String, v: HWND) {
    GLOBAL_HWND.lock().unwrap().insert(k, v);
}

static GLOBAL_PREF: Lazy<Mutex<HashMap<String, i32>>> = Lazy::new(|| Mutex::new(HashMap::new()));

fn set_global_pref(k: &str, v: i32) {
    GLOBAL_PREF.lock().unwrap().insert(k.to_string(), v);
}

#[derive(Debug)]
struct TccGpu {
    id: i32,  // gpu_index
    name: String,
    luid: String,
    regex_luid: Regex,
    sum_value: f64
}

#[derive(Debug, Default)]
struct TccPerformanceCounter {
    id: String,  // performance_counter_name
    gpu_index: i32,
    handle: isize,
    hit: bool
}

static mut GLOBAL_END: bool = false;
static mut GLOBAL_UTC_NOW: Lazy<DateTime<Utc>> = Lazy::new(|| Utc::now());
static mut GLOBAL_MENU_HANDLE: Lazy<HMENU> = Lazy::new(|| HMENU::default());

fn main() -> anyhow::Result<()> {
    unsafe {

        // wait for reload
        let args: Vec<String> = env::args().collect();
        if args.len() == 2 {
            let pid: u32 = args.get_unchecked(1).parse().unwrap(); 
            match Win32::System::Threading::OpenProcess(
                PROCESS_ACCESS_RIGHTS(0x00100000),
                FALSE,
                pid
            ) {
                std::result::Result::Ok(h) => {
                    let ret = WaitForSingleObject(h.0, 10 * 1000);
                    if ret == WAIT_TIMEOUT.0 {
                        println!("error 001");
                    }
                },
                Err(_) => {
                    println!("error 002");
                },
            }
        }

        // right click menu
        *GLOBAL_MENU_HANDLE = CreatePopupMenu().unwrap();
        AppendMenuW(
            *GLOBAL_MENU_HANDLE,
            MF_STRING,
            2,
            PCWSTR(convert_utf16_null("Reload").as_ptr())
        );
        AppendMenuW(
            *GLOBAL_MENU_HANDLE,
            MF_STRING,
            1,
            PCWSTR(convert_utf16_null("Exit").as_ptr())
        );
    
        // load config
        let mut config_path = env::current_exe().unwrap();
        config_path.pop();
        config_path.push("config.txt");
        let mut config: serde_json::Value = match fs::read_to_string(config_path) {
            Err(err) => {
                error_messagebox("file open config.txt", &err.to_string());
                panic!("{}", err.to_string())
            }
            std::result::Result::Ok(read_text) => {
                match serde_json::from_str(&read_text.replace("\\", "\\\\")) {
                    Err(err) => {
                        error_messagebox("load config.txt", &err.to_string());
                        panic!("{}", err.to_string())
                    }
                    std::result::Result::Ok(v) => {v}
                }
            }
        };

        if config["custom_formats"] != serde_json::Value::Null && config["custom_formats"].is_array() {
            let mut global_tcc_custom_format_hm = GLOBAL_TCC_CUSTOM_FORMAT.lock().unwrap();
            for config_custom_format in config["custom_formats"].as_array_mut().unwrap().iter() {
                let mut tcc_custom_format = TccCustomFormat::default();
                match &config_custom_format["spec"] {
                    serde_json::Value::Null => {
                        continue;
                    }
                    value => {
                        let s = value.as_str().unwrap();
                        if s == "" {
                            continue;
                        } else {
                            tcc_custom_format.spec = s.to_string();
                        }
                    }
                }
                match &config_custom_format["value"] {
                    serde_json::Value::Null => {
                        continue;
                    }
                    value => {
                        let s = value.as_str().unwrap();
                        if s == "" {
                            continue;
                        } else {
                            tcc_custom_format.value = s.to_string();
                        }
                    }
                }
                match &config_custom_format["replace"] {
                    serde_json::Value::Null => {
                        continue;
                    }
                    value => {
                        for tpl in value.as_object().unwrap().iter() {
                            if !tcc_custom_format.items.contains_key(tpl.0) {
                                tcc_custom_format.items.insert(tpl.0.to_string() , tpl.1.as_str().unwrap().to_string());
                            }
                        }
                    }
                }
                if !global_tcc_custom_format_hm.contains_key(&tcc_custom_format.spec) {
                    global_tcc_custom_format_hm.insert(tcc_custom_format.spec.clone(), tcc_custom_format);
                }
            }
        }

        // init create tcc-win11 window
        let instance = GetModuleHandleW(None).unwrap();
        let class_name = convert_utf16_null("tcc_win11_window_class");
        let wc = WNDCLASSW {
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap(),
            hInstance: instance,
            lpszClassName: PCWSTR(class_name.as_ptr()),
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            ..Default::default()
        };
        RegisterClassW(&wc);

        let mut vec_window_hwnd: Vec<HWND> = Vec::new();

        // get all display info

        let p = LPARAM::default();
        EnumDisplayMonitors(
            None,
            None,
            Some(enumerate_callback_get_monitors_info),
            p
        );

        let mut path_count: u32 = 0;
        let mut mode_count: u32 = 0;
        let ret = GetDisplayConfigBufferSizes(
            QDC_ONLY_ACTIVE_PATHS,
            &mut path_count,
            &mut mode_count
        );
        if ret != NO_ERROR {
            error_messagebox("GetDisplayConfigBufferSizes", "");
            return Err(anyhow!("Error GetDisplayConfigBufferSizes"));
        }

        let mut path_array:Vec<DISPLAYCONFIG_PATH_INFO> = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
        let mut mode_array:Vec<DISPLAYCONFIG_MODE_INFO> = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
        let ret = QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut path_count,
            &mut path_array[0],
            &mut mode_count,
            &mut mode_array[0],
            None
        );
        if ret != NO_ERROR {
            error_messagebox("QueryDisplayConfig", "");
            return Err(anyhow!("Error QueryDisplayConfig"));
        }

        for i in 0..path_count {
            let mut displayconfig_target = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
            displayconfig_target.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
            displayconfig_target.header.size = mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
            displayconfig_target.header.adapterId = path_array[i as usize].targetInfo.adapterId;
            displayconfig_target.header.id = path_array[i as usize].targetInfo.id;
            let ret = DisplayConfigGetDeviceInfo(&mut displayconfig_target.header);
            if ret != 0 {
                continue;
            }
            let device_path = displayconfig_target.monitorDevicePath.iter().map(|x| (char::from_u32(*x as u32)).unwrap()).collect::<String>();

            let mut displayconfig_source = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            displayconfig_source.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
            displayconfig_source.header.size = mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
            displayconfig_source.header.adapterId = path_array[i as usize].targetInfo.adapterId;
            displayconfig_source.header.id = path_array[i as usize].sourceInfo.id;
            let ret = DisplayConfigGetDeviceInfo(&mut displayconfig_source.header);
            if ret != 0 {
                continue;
            }
            let device_name = displayconfig_source.viewGdiDeviceName.iter().map(|x| (char::from_u32(*x as u32)).unwrap()).collect::<String>();

            let mut wk = device_path.clone();
            wk.remove(0);
            wk.remove(0);
            wk.remove(0);
            wk.remove(0);
            loop {
                match wk.pop() {
                    None => {
                        break;
                    }
                    Some('#') => {
                        break;
                    }
                    Some(_) => {
                        continue;
                    }
                }
            }
            wk = wk.replace("#", "\\").to_uppercase();

            {
                let mut global_tcc_display_hm = GLOBAL_TCC_DISPLAY.lock().unwrap();
                global_tcc_display_hm.get_mut(&device_name).unwrap().device_path = wk;
            }
        }

        // get main display taskbar info

        // get taskbar hwnd
        let taskbar_classname = convert_utf16_null("Shell_TrayWnd");
        let taskbar_windowname = convert_utf16_null("");
        let mut taskbar_hwnd;
        loop {
            taskbar_hwnd = FindWindowW(
                PCWSTR(taskbar_classname.as_ptr()),
                PCWSTR(taskbar_windowname.as_ptr())
            );
            if taskbar_hwnd == HWND::default() {
                sleep(core::time::Duration::from_millis(1000));
            } else {
                break;                
            }
        }
        // get taskbar content hwnd
        let taskbar_content_hwnd: HWND;
        loop {
            EnumChildWindows(
                taskbar_hwnd,
                Some(enumerate_callback_get_taskbar_content_hwnd),
                None
            );
            {
                let mut global_hwnd_hm = GLOBAL_HWND.lock().unwrap();
                if global_hwnd_hm.contains_key("taskbar_content_hwnd") {
                    taskbar_content_hwnd = *global_hwnd_hm.get("taskbar_content_hwnd").unwrap();
                    global_hwnd_hm.clear();
                    break;
                } else {
                    sleep(core::time::Duration::from_millis(1000));
                }
            }    
        }

        // get taskbar rect
        let mut taskbar_rect = RECT::default();
        GetWindowRect(taskbar_hwnd, &mut taskbar_rect);
        {
            let mut global_tcc_display_hm = GLOBAL_TCC_DISPLAY.lock().unwrap();
            for ref_global_tcc_display in  global_tcc_display_hm.values_mut() {
                if ref_global_tcc_display.display_rect.left <= taskbar_rect.left
                && taskbar_rect.left < ref_global_tcc_display.display_rect.right
                && ref_global_tcc_display.display_rect.top <= taskbar_rect.top
                && taskbar_rect.top < ref_global_tcc_display.display_rect.bottom {

                    ref_global_tcc_display.taskbar_tray_hwnd = taskbar_hwnd;
                    ref_global_tcc_display.taskbar_content_hwnd = taskbar_content_hwnd;
                    ref_global_tcc_display.taskbar_rect = taskbar_rect;

                    let mut display_target = String::from("");
                    let mut display_id = "";
                    let mut config_display = serde_json::Value::Null;
                    let config_displays = config["displays"].as_array().unwrap();
                    for c in config_displays {
                        display_target = ref_global_tcc_display.device_path.to_string();
                        display_id = &ref_global_tcc_display.device_path;
                        if c["target"].as_str().unwrap() == display_id {
                            config_display = c.clone();
                            break;
                        }
                    }
                    if config_display == serde_json::Value::Null {
                        for c in config_displays {
                            display_target = "main".to_string();
                            display_id = "main";
                            if c["target"].as_str().unwrap() == display_id {
                                config_display = c.clone();
                                break;
                            }
                        }
                    }
                    if config_display == serde_json::Value::Null {
                        for c in config_displays {
                            display_target = "all".to_string();
                            display_id = "all_0";
                            if c["target"].as_str().unwrap() == "all" {
                                config_display = c.clone();
                                break;
                            }
                        }
                    }
                    if config_display != serde_json::Value::Null {
                        ref_global_tcc_display.id = display_id.to_string();
                        ref_global_tcc_display.taskbar_adjust_left = 0;
                        ref_global_tcc_display.taskbar_adjust_right = 0;
                        if config_display["taskbar_adjust"] != serde_json::Value::Null {
                            let config_value = &config_display["taskbar_adjust"]["left"];
                            if *config_value != serde_json::Value::Null && config_value.is_i64() {
                                let val = config_value.as_f64().unwrap() * ref_global_tcc_display.zoom;
                                ref_global_tcc_display.taskbar_adjust_left = val.ceil() as i32;
                            }
                            let config_value = &config_display["taskbar_adjust"]["right"];
                            if *config_value != serde_json::Value::Null && config_value.is_i64() {
                                let val = config_value.as_f64().unwrap() * ref_global_tcc_display.zoom;
                                ref_global_tcc_display.taskbar_adjust_right = val.ceil() as i32;
                            }
                        }

                        match create_window(wc, ref_global_tcc_display, config_display) {
                            Err(err) => {
                                error_messagebox(&format!("create window : target = {display_target}"), &err.to_string());
                                return Err(err);
                            }
                            anyhow::Result::Ok(_) => {
                                for p in ref_global_tcc_display.panels.iter_mut() {
                                    vec_window_hwnd.push(p.1.hwnd);
                                }
                            }
                        }
                    }
                    break;
                }
            }

        }

        // get sub displays taskbar info

        let mut taskbar_hwnd = HWND::default();
        let mut sub_counter = 0;

        loop {
            sub_counter += 1;
            // get taskbar hwnd
            let taskbar_classname = convert_utf16_null("Shell_SecondaryTrayWnd");
            taskbar_hwnd = FindWindowExW(
                None,
                taskbar_hwnd,
                PCWSTR(taskbar_classname.as_ptr()),
                None
            );
            match taskbar_hwnd.0 {
                0 => { break }
                _ => {}
            }
            // get taskbar content hwnd
            let taskbar_content_hwnd: HWND;
            loop {
                EnumChildWindows(
                    taskbar_hwnd,
                    Some(enumerate_callback_get_taskbar_content_hwnd),
                    None
                );
                {
                    let mut global_hwnd_hm = GLOBAL_HWND.lock().unwrap();
                    if global_hwnd_hm.contains_key("taskbar_content_hwnd") {
                        taskbar_content_hwnd = *global_hwnd_hm.get("taskbar_content_hwnd").unwrap();
                        global_hwnd_hm.clear();
                        break;
                    } else {
                        sleep(core::time::Duration::from_millis(1000));
                    }
                }    
            }
    
            // get taskbar rect
            let mut taskbar_rect = RECT::default();
            GetWindowRect(taskbar_hwnd, &mut taskbar_rect);
            {
                let mut global_tcc_display_hm = GLOBAL_TCC_DISPLAY.lock().unwrap();
                for ref_global_tcc_display in global_tcc_display_hm.values_mut() {
                    if ref_global_tcc_display.display_rect.left <= taskbar_rect.left
                    && taskbar_rect.left < ref_global_tcc_display.display_rect.right
                    && ref_global_tcc_display.display_rect.top <= taskbar_rect.top
                    && taskbar_rect.top < ref_global_tcc_display.display_rect.bottom {

                        ref_global_tcc_display.taskbar_tray_hwnd = taskbar_hwnd;
                        ref_global_tcc_display.taskbar_content_hwnd = taskbar_content_hwnd;
                        ref_global_tcc_display.taskbar_rect = taskbar_rect;

                        let mut display_target = String::from("");
                        let mut display_id = "".to_string();
                        let mut config_display = serde_json::Value::Null;
                        let config_displays = config["displays"].as_array().unwrap();
                        for c in config_displays {
                            display_target = ref_global_tcc_display.device_path.to_string();
                            display_id = ref_global_tcc_display.device_path.clone();
                            if c["target"].as_str().unwrap() == display_id {
                                config_display = c.clone();
                                break;
                            }
                        }
                        if config_display == serde_json::Value::Null {
                            for c in config_displays {
                                display_target = "sub".to_string();
                                display_id = "sub_".to_string() + &sub_counter.to_string();
                                if c["target"].as_str().unwrap() == "sub" {
                                    config_display = c.clone();
                                    break;
                                }
                            }
                        }
                        if config_display == serde_json::Value::Null {
                            for c in config_displays {
                                display_target = "all".to_string();
                                display_id = "all_".to_string() + &sub_counter.to_string();
                                if c["target"].as_str().unwrap() == "all" {
                                    config_display = c.clone();
                                    break;
                                }
                            }
                        }
                        if config_display != serde_json::Value::Null {
                            ref_global_tcc_display.id = display_id;
                            ref_global_tcc_display.taskbar_adjust_left = 0;
                            ref_global_tcc_display.taskbar_adjust_right = 0;
                            if config_display["taskbar_adjust"] != serde_json::Value::Null {
                                let config_value = &config_display["taskbar_adjust"]["left"];
                                if *config_value != serde_json::Value::Null && config_value.is_i64() {
                                    let val = config_value.as_f64().unwrap() * ref_global_tcc_display.zoom;
                                    ref_global_tcc_display.taskbar_adjust_left = val.ceil() as i32;
                                }
                                let config_value = &config_display["taskbar_adjust"]["right"];
                                if *config_value != serde_json::Value::Null && config_value.is_i64() {
                                    let val = config_value.as_f64().unwrap() * ref_global_tcc_display.zoom;
                                    ref_global_tcc_display.taskbar_adjust_right = val.ceil() as i32;
                                }
                            }

                            match create_window(wc, ref_global_tcc_display, config_display) {
                                Err(err) => {
                                    error_messagebox(&format!("create window : target = {display_target}"), &err.to_string());
                                    return Err(err);
                                }
                                anyhow::Result::Ok(_) => {
                                    for p in ref_global_tcc_display.panels.iter_mut() {
                                        vec_window_hwnd.push(p.1.hwnd);
                                    }
                                }
                            }
                        }
                        break;
                    }
                }
            }
        }

        // performance counter
        let mut is_get_pref_cpu = false;
        let mut is_get_pref_gpu = false;
        let mut is_get_pref_mem = false;
        let re_cpu = Regex::new(r"\{(cpu|-cpu|_cpu|0cpu)\}").unwrap();
        let re_gpu = Regex::new(r"\{(gpu|-gpu|_gpu|0gpu)([0-9]+)\}").unwrap();
        let re_mem = Regex::new(r"\{(mem|-mem|_mem|0mem)\}").unwrap();
        {
            let global_tcc_display_hm = GLOBAL_TCC_DISPLAY.lock().unwrap();
            for global_tcc_display in global_tcc_display_hm.values() {
                for panel in global_tcc_display.panels.values() {
                    for label in panel.labels.iter() {
                        if !is_get_pref_cpu && re_cpu.is_match(&label.format) {
                            is_get_pref_cpu = true;
                        }
                        if !is_get_pref_gpu && re_gpu.is_match(&label.format) {
                            is_get_pref_gpu = true;
                        }
                        if !is_get_pref_mem && re_mem.is_match(&label.format) {
                            is_get_pref_mem = true;
                        }
                        if is_get_pref_cpu && is_get_pref_gpu && is_get_pref_mem {
                            break;
                        }
                    }
                    if is_get_pref_cpu && is_get_pref_gpu && is_get_pref_mem {
                        break;
                    }
                }
                if is_get_pref_cpu && is_get_pref_gpu && is_get_pref_mem {
                    break;
                }
            }
        }

        // init cpu performance counter
        let mut pdh_query_handle: isize = 0;
        if is_get_pref_cpu || is_get_pref_gpu {
            PdhOpenQueryW(
                None,
                0,
                &mut pdh_query_handle
            );
        }

        let mut cpu_pdh_counter_handle: isize = 0;
        if is_get_pref_cpu {
            let counter_path = r#"\Processor Information(_Total)\% Processor Utility"#;
            PdhAddCounterW(
                pdh_query_handle,
                PCWSTR(convert_utf16_null(&counter_path).as_ptr()),
                0,
                &mut cpu_pdh_counter_handle
            );
        }

        let mut tcc_gpu_vec: Vec<TccGpu> = Vec::new();
        let mut tcc_performance_counter_hm: HashMap<String, TccPerformanceCounter> = HashMap::new();
        if is_get_pref_gpu {
            // get gpu info
            let com_con = COMLibrary::new()?;
            let wmi_con = WMIConnection::new(com_con.into())?;
            let wmi_results: Vec<HashMap<String, Variant>> = wmi_con.raw_query("SELECT Name, PNPDeviceID FROM Win32_VideoController").unwrap();
            let mut gpu_index = 0;
            for wmi_result in wmi_results {
                let mut tcc_gpu = TccGpu {
                    id: gpu_index,
                    name: "".to_string(),
                    luid: "".to_string(),
                    regex_luid: Regex::new("").unwrap(),
                    sum_value: 0.0
                };
                match &wmi_result["Name"] {
                    Variant::String(x) => {
                        tcc_gpu.name = x.to_string();
                    },
                    _ => {
                        // nop
                    },
                }
                match &wmi_result["PNPDeviceID"] {
                    Variant::String(x) => {
                        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
                        let path = r#"SYSTEM\CurrentControlSet\Enum\"#.to_string() + x + r#"\Device Parameters"#;
                        let key = hklm.open_subkey(path)?;
                        let val: String = key.get_value("VideoID")?;
                        let path = r#"SOFTWARE\Microsoft\DirectX\"#.to_string() + &val;
                        let key = hklm.open_subkey(path)?;
                        let val: u64 = key.get_value("AdapterLuid")?;
                        let luid = format!("{:0>8}", format!("{:X}", val));
                        tcc_gpu.luid = format!("_luid_0x00000000_0x{}_", luid);
                        tcc_gpu.regex_luid = Regex::new(&tcc_gpu.luid).unwrap();
                    },
                    _ => {
                        // nop
                    },
                }
                tcc_gpu_vec.push(tcc_gpu);
                gpu_index += 1;
            }

            // init gpu performance counter
            let pcchbuffersize: *mut u32 = &mut 0;
            let mszobjectlist= PWSTR::null();
            PdhEnumObjectsW(
                None,
                None,
                mszobjectlist,
                pcchbuffersize,
                PERF_DETAIL_WIZARD,
                TRUE
            );
            let mszcounterlist= PWSTR::null();
            let pcchcounterlistlength: *mut u32 = &mut 0;
            let mszinstancelist= PWSTR::null();
            let pcchinstancelistlength: *mut u32 = &mut 0;
            PdhEnumObjectItemsW(
                None,
                None,
                PCWSTR(convert_utf16_null("GPU Engine").as_ptr()),
                mszcounterlist,
                pcchcounterlistlength,
                mszinstancelist,
                pcchinstancelistlength,
                PERF_DETAIL_WIZARD,
                0
            );
            let mut mszcounterlist = vec![0u16; *pcchcounterlistlength as usize];
            let mut mszinstancelist = vec![0u16; *pcchinstancelistlength as usize];
            PdhEnumObjectItemsW(
                None,
                None,
                PCWSTR(convert_utf16_null("GPU Engine").as_ptr()),
                PWSTR(mszcounterlist.as_mut_ptr()),
                pcchcounterlistlength,
                PWSTR(mszinstancelist.as_mut_ptr()),
                pcchinstancelistlength,
                PERF_DETAIL_WIZARD,
                0
            );
            let mut perf_instance_name_vec: Vec<String> = Vec::new();
            let re = Regex::new(r"(.*?)\x00").unwrap();
            for caps in re.captures_iter(&String::from_utf16(&mszinstancelist).unwrap()) {
                perf_instance_name_vec.push(caps[1].to_string());
            }
            for perf_instance_name in perf_instance_name_vec {
                for i in 0..tcc_gpu_vec.len() {
                    let re = Regex::new(&tcc_gpu_vec[i].luid).unwrap();
                    if re.is_match(&perf_instance_name) {
                        let mut tcc_performance_counter = TccPerformanceCounter::default();
                        tcc_performance_counter.id = perf_instance_name;
                        tcc_performance_counter.gpu_index = i as i32;
                        let counter_path = format!(r#"\GPU Engine({})\Utilization Percentage"#, &tcc_performance_counter.id);
                        PdhAddCounterW(
                            pdh_query_handle,
                            PCWSTR(convert_utf16_null(&counter_path).as_ptr()),
                            0,
                            &mut tcc_performance_counter.handle
                        );
                        tcc_performance_counter_hm.insert(tcc_performance_counter.id.to_string(), tcc_performance_counter);
                        break;
                    }
                }
            }
        }
        if is_get_pref_cpu || is_get_pref_gpu {
            PdhCollectQueryData(pdh_query_handle);
        }

        // taskbar check thread
        let mut tcc_taskbar_checker_vec: Vec<TccTaskbarChecker> = Vec::new();
        {
            let global_tcc_display_hm = GLOBAL_TCC_DISPLAY.lock().unwrap();
            for global_tcc_display in global_tcc_display_hm.values() {
                let mut tcc_taskbar_checker = TccTaskbarChecker::default();
                tcc_taskbar_checker.display_rect = global_tcc_display.display_rect;
                tcc_taskbar_checker.taskbar_rect = global_tcc_display.taskbar_rect;
                tcc_taskbar_checker.taskbar_adjust_left = global_tcc_display.taskbar_adjust_left;
                tcc_taskbar_checker.taskbar_adjust_right = global_tcc_display.taskbar_adjust_right;
                tcc_taskbar_checker.taskbar_tray_hwnd = global_tcc_display.taskbar_tray_hwnd;
                tcc_taskbar_checker.taskbar_content_hwnd = global_tcc_display.taskbar_content_hwnd;
                tcc_taskbar_checker.taskbar_topmost = false;
                tcc_taskbar_checker.taskbar_visible = -1;

                tcc_taskbar_checker.display_id = global_tcc_display.device_name.to_string();
                for v in global_tcc_display.panels.values() {
                    tcc_taskbar_checker.panel_id_vec.push((v.id.to_string(), v.hwnd));
                    tcc_taskbar_checker.panel_background_init = v.background_init;
                }
                tcc_taskbar_checker_vec.push(tcc_taskbar_checker);
            }
        }
        thread::spawn(move || {
            loop {
                for ref_tcc_taskbar_checker in tcc_taskbar_checker_vec.iter_mut() {

                    if !ref_tcc_taskbar_checker.panel_background_init {
                        let style = GetWindowLongW(ref_tcc_taskbar_checker.taskbar_tray_hwnd, GWL_STYLE) as u32;
                        let exstyle = GetWindowLongW(ref_tcc_taskbar_checker.taskbar_tray_hwnd, GWL_EXSTYLE) as u32;
                        let mut rect = RECT::default();
                        GetWindowRect(ref_tcc_taskbar_checker.taskbar_tray_hwnd, &mut rect);
                        if (exstyle & WS_EX_TOPMOST.0 == WS_EX_TOPMOST.0) && (style & WS_VISIBLE.0 == WS_VISIBLE.0) && (ref_tcc_taskbar_checker.display_rect.bottom == rect.bottom) && !is_lock_screen() {

                            sleep(core::time::Duration::from_millis(1000));

                            ref_tcc_taskbar_checker.panel_background_init = true;

                            SetWindowPos(
                                ref_tcc_taskbar_checker.taskbar_content_hwnd,
                                HWND_BOTTOM,
                                0,
                                0,
                                ref_tcc_taskbar_checker.display_rect.right - ref_tcc_taskbar_checker.display_rect.left,
                                ref_tcc_taskbar_checker.taskbar_rect.bottom - ref_tcc_taskbar_checker.taskbar_rect.top,
                                SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOSENDCHANGING | SWP_NOMOVE
                            );

                            for panel_id_hwnd in ref_tcc_taskbar_checker.panel_id_vec.iter_mut() {
                                ShowWindow(panel_id_hwnd.1, SW_HIDE);
                            }

                            {
                                let mut global_tcc_display_hm = GLOBAL_TCC_DISPLAY.lock().unwrap();
                                match global_tcc_display_hm.get_mut(&ref_tcc_taskbar_checker.display_id) {
                                    None => {
                                    }
                                    Some(global_tcc_display) => {
                                        for panel_id_hwnd in ref_tcc_taskbar_checker.panel_id_vec.iter_mut() {
                                            let mut panel_rect = RECT::default();
                                            GetWindowRect(panel_id_hwnd.1, &mut panel_rect);
                                            let tcc_panel = global_tcc_display.panels.get_mut(&panel_id_hwnd.0).unwrap();
                                            let window_height = ref_tcc_taskbar_checker.taskbar_rect.bottom - ref_tcc_taskbar_checker.taskbar_rect.top;
                                            let hdc = GetDC(tcc_panel.hwnd);
                                            tcc_panel.background_dc = CreateCompatibleDC(hdc);
                                            tcc_panel.background_bm = CreateCompatibleBitmap(hdc, tcc_panel.width, window_height);
                                            SelectObject(tcc_panel.background_dc, tcc_panel.background_bm);
    
                                            let desktop_hdc = GetDC(HWND(0));
                                            for i in 0..window_height {
                                                let j = match i {
                                                    0 => 0,
                                                    _ => window_height - 1
                                                };
                                                BitBlt(
                                                    tcc_panel.background_dc,
                                                    0,
                                                    i,
                                                    tcc_panel.width,
                                                    1,
                                                    desktop_hdc,
                                                    panel_rect.left,
                                                    panel_rect.top + j,
                                                    SRCCOPY
                                                );
                                            }
                                            tcc_panel.background_init = true;
                                        }
                                    }
                                }    
                            }

                            for panel_id_hwnd in ref_tcc_taskbar_checker.panel_id_vec.iter_mut() {
                                ShowWindow(panel_id_hwnd.1, SW_HIDE);
                            }

                            ref_tcc_taskbar_checker.taskbar_visible = -1;
                        }
                    }

                    let exstyle = GetWindowLongW(ref_tcc_taskbar_checker.taskbar_tray_hwnd, GWL_EXSTYLE) as u32;
                    if exstyle & WS_EX_TOPMOST.0 == 0 {
                        if ref_tcc_taskbar_checker.taskbar_topmost == true {
                            for panel_id_hwnd in ref_tcc_taskbar_checker.panel_id_vec.iter_mut() {
                                SetWindowPos(
                                    panel_id_hwnd.1,
                                    HWND_BOTTOM,
                                    0,
                                    0,
                                    0,
                                    0,
                                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE
                                );
                            }
                        }
                        ref_tcc_taskbar_checker.taskbar_topmost = false;
                    } else {
                        if ref_tcc_taskbar_checker.taskbar_topmost == false {
                            for panel_id_hwnd in ref_tcc_taskbar_checker.panel_id_vec.iter_mut() {
                                SetWindowPos(
                                    panel_id_hwnd.1,
                                    HWND_TOPMOST,
                                    0,
                                    0,
                                    0,
                                    0,
                                    SWP_NOMOVE | SWP_NOSIZE
                                );
                            }
                        } else {
                            for panel_id_hwnd in ref_tcc_taskbar_checker.panel_id_vec.iter_mut() {
                                let mut h = panel_id_hwnd.1;
                                loop {
                                    h = GetWindow(h, GW_HWNDPREV);
                                    if h.0 == 0 {
                                        break;
                                    }
                                    if h == ref_tcc_taskbar_checker.taskbar_tray_hwnd {
                                        SetWindowPos(
                                            panel_id_hwnd.1,
                                            HWND_TOPMOST,
                                            0,
                                            0,
                                            0,
                                            0,
                                            SWP_NOMOVE | SWP_NOSIZE
                                        );
                                        break;
                                    }
                                }
                            }
                        }
                        ref_tcc_taskbar_checker.taskbar_topmost = true;
                    }

                    let mut rect = RECT::default();
                    GetWindowRect(ref_tcc_taskbar_checker.taskbar_tray_hwnd, &mut rect);
                    if ref_tcc_taskbar_checker.display_rect.bottom == rect.bottom {
                        if ref_tcc_taskbar_checker.taskbar_visible == -1 || ref_tcc_taskbar_checker.taskbar_visible == 0 {
                            ref_tcc_taskbar_checker.taskbar_visible = 1;
                            for panel_id_hwnd in ref_tcc_taskbar_checker.panel_id_vec.iter_mut() {
                                ShowWindow(panel_id_hwnd.1, SW_SHOW);
                            }
                        }
                    } else {
                        if ref_tcc_taskbar_checker.taskbar_visible == -1 || ref_tcc_taskbar_checker.taskbar_visible == 1 {
                            ref_tcc_taskbar_checker.taskbar_visible = 0;
                            for panel_id_hwnd in ref_tcc_taskbar_checker.panel_id_vec.iter_mut() {
                                ShowWindow(panel_id_hwnd.1, SW_HIDE);
                            }
                        }
                    }

                    if ref_tcc_taskbar_checker.taskbar_adjust_left != 0 || ref_tcc_taskbar_checker.taskbar_adjust_right != 0 {
                        let mut rect = RECT::default();
                        GetWindowRect(ref_tcc_taskbar_checker.taskbar_content_hwnd, &mut rect);
                        let taskbar_width = rect.right - rect.left;
                        let display_width = ref_tcc_taskbar_checker.display_rect.right - ref_tcc_taskbar_checker.display_rect.left;
                        if taskbar_width == display_width {
                            SetWindowPos(
                                ref_tcc_taskbar_checker.taskbar_content_hwnd,
                                HWND_BOTTOM,
                                ref_tcc_taskbar_checker.taskbar_adjust_left,
                                0,
                                display_width - ref_tcc_taskbar_checker.taskbar_adjust_left + ref_tcc_taskbar_checker.taskbar_adjust_right,
                                ref_tcc_taskbar_checker.taskbar_rect.bottom - ref_tcc_taskbar_checker.taskbar_rect.top,
                                SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOSENDCHANGING
                            );
                        }
                    }
                }
                sleep(core::time::Duration::from_millis(100));
            }
        });

        // clock timer thread
        let regex_perf_instance_name = Regex::new(r"(.*?)\x00").unwrap();
        let (clock_thread_channel_sender, clock_thread_channel_receiver) = channel();
        let clock_thread_join_handle = spawn(move || {
            loop {
                *GLOBAL_UTC_NOW = Utc::now();

                match clock_thread_channel_receiver.try_recv() {
                    anyhow::Result::Ok(1) => break,
                    _ => ()
                }

                // Pref
                if is_get_pref_cpu || is_get_pref_gpu {
                    PdhCollectQueryData(pdh_query_handle);
                }

                // Pref CPU
                if is_get_pref_cpu {
                    let mut counter_value = PDH_FMT_COUNTERVALUE::default();
                    let ret = PdhGetFormattedCounterValue(
                        cpu_pdh_counter_handle,
                        PDH_FMT_DOUBLE, //PDH_FMT_DOUBLE, PDH_FMT_LARGE, PDH_FMT_LONG
                        None,
                        &mut counter_value
                    );
                    if ret == 0 {
                        let mut value = &counter_value.Anonymous.doubleValue;
                        if value > &100.0 {
                            value = &100.0;
                        }
                        set_global_pref("cpu", value.round() as i32);
                    } else {
                        set_global_pref("cpu", 0);
                    }
                }

                // Pref GPU
                if is_get_pref_gpu {
                    for tcc_gpu in tcc_gpu_vec.iter_mut() {
                        tcc_gpu.sum_value = 0.0;
                    }
                    for tcc_performance_counter in tcc_performance_counter_hm.values() {
                        let mut v = PDH_FMT_COUNTERVALUE::default();
                        let ret = PdhGetFormattedCounterValue(
                            tcc_performance_counter.handle,
                            PDH_FMT_DOUBLE, //PDH_FMT_DOUBLE, PDH_FMT_LARGE, PDH_FMT_LONG
                            None,
                            &mut v
                        );
                        if ret == 0 {
                            let tcc_gpu = &mut tcc_gpu_vec[tcc_performance_counter.gpu_index as usize];
                            tcc_gpu.sum_value += &v.Anonymous.doubleValue;
                        }
                    }
                    for tcc_gpu in tcc_gpu_vec.iter() {
                        let mut value = tcc_gpu.sum_value.round();
                        if value > 100.0 {
                            value = 100.0;
                        }
                        set_global_pref(&format!("gpu{}", tcc_gpu.id), value as i32);
                    }

                    let mszcounterlist= PWSTR::null();
                    let pcchcounterlistlength: *mut u32 = &mut 0;
                    let mszinstancelist= PWSTR::null();
                    let pcchinstancelistlength: *mut u32 = &mut 0;
                    PdhEnumObjectItemsW(
                        None,
                        None,
                        PCWSTR(convert_utf16_null("GPU Engine").as_ptr()),
                        mszcounterlist,
                        pcchcounterlistlength,
                        mszinstancelist,
                        pcchinstancelistlength,
                        PERF_DETAIL_WIZARD,
                        0
                    );

                    let mut mszcounterlist = vec![0u16; *pcchcounterlistlength as usize];
                    let mut mszinstancelist = vec![0u16; *pcchinstancelistlength as usize];
                    PdhEnumObjectItemsW(
                        None,
                        None,
                        PCWSTR(convert_utf16_null("GPU Engine").as_ptr()),
                        PWSTR(mszcounterlist.as_mut_ptr()),
                        pcchcounterlistlength,
                        PWSTR(mszinstancelist.as_mut_ptr()),
                        pcchinstancelistlength,
                        PERF_DETAIL_WIZARD,
                        0
                    );

                    let mut perf_instance_name_vec: Vec<String> = Vec::new();
                    for caps in regex_perf_instance_name.captures_iter(&String::from_utf16(&mszinstancelist).unwrap()) {
                        perf_instance_name_vec.push(caps[1].to_string());
                    }

                    for x in tcc_performance_counter_hm.values_mut() {
                        x.hit = false;
                    }
                    let mut is_counter_add = false;
                    for perf_instance_name in perf_instance_name_vec {
                        for i in 0..tcc_gpu_vec.len() {
                            if tcc_gpu_vec[i].regex_luid.is_match(&perf_instance_name) {
                                if tcc_performance_counter_hm.contains_key(&perf_instance_name) {
                                    tcc_performance_counter_hm.get_mut(&perf_instance_name).unwrap().hit = true;
                                } else {
                                    let mut tcc_performance_counter = TccPerformanceCounter::default();
                                    tcc_performance_counter.id = perf_instance_name;
                                    tcc_performance_counter.gpu_index = i as i32;
                                    tcc_performance_counter.hit = true;
                                    let counter_path = format!(r#"\GPU Engine({})\Utilization Percentage"#, &tcc_performance_counter.id);
                                    PdhAddCounterW(
                                        pdh_query_handle,
                                        PCWSTR(convert_utf16_null(&counter_path).as_ptr()),
                                        0,
                                        &mut tcc_performance_counter.handle
                                    );
                                    tcc_performance_counter_hm.insert(tcc_performance_counter.id.to_string(), tcc_performance_counter);
                                    is_counter_add = true;
                                }
                                break;
                            }
                        }
                    }
                    let mut v: Vec<String> = Vec::new();
                    for x in tcc_performance_counter_hm.values() {
                        if !x.hit {
                            PdhRemoveCounter(x.handle);
                            v.push(x.id.to_string());
                        }
                    }
                    for x in v {
                        tcc_performance_counter_hm.remove(&x);
                    }
                    if is_counter_add {
                        PdhCollectQueryData(pdh_query_handle);
                    }
                }

                // MEM
                if is_get_pref_mem {
                    let com_con = COMLibrary::new().unwrap();
                    let wmi_con = WMIConnection::new(com_con.into()).unwrap();
                    let wmi_results: Vec<HashMap<String, Variant>> = wmi_con.raw_query("SELECT TotalVisibleMemorySize, FreePhysicalMemory FROM Win32_OperatingSystem").unwrap();
                    let mut total_memory: f64 = 0.0;
                    match wmi_results[0]["TotalVisibleMemorySize"] {
                        Variant::UI8(x) => {
                            total_memory = x as f64;
                        },
                        _ => {
                            // nop
                        },
                    }
                    let mut free_memory: f64 = 0.0;
                    match wmi_results[0]["FreePhysicalMemory"] {
                        Variant::UI8(x) => {
                            free_memory = x as f64;
                        },
                        _ => {
                            // nop
                        },
                    }
                    set_global_pref("mem", ((1.0 - free_memory / total_memory) * 100.0).round() as i32);
                }

                match clock_thread_channel_receiver.try_recv() {
                    anyhow::Result::Ok(1) => break,
                    _ => ()
                }

                // refresh all window
                for h in vec_window_hwnd.iter() {
                    InvalidateRect(*h, None, true);
                    UpdateWindow(*h);
                }

                // timer
                let local: DateTime<Local> = Local::now();
                let dur: u64 = (local.nanosecond() / 1000000) as u64;
                thread::sleep(core::time::Duration::from_millis(1000 - dur));
            }

            // clean up pdh
            if is_get_pref_cpu {
                PdhRemoveCounter(cpu_pdh_counter_handle);
            }
            if is_get_pref_gpu {
                for x in tcc_performance_counter_hm.values() {
                    PdhRemoveCounter(x.handle);
                }
            }
            if is_get_pref_cpu || is_get_pref_gpu {
                PdhCloseQuery(pdh_query_handle);
            }

        });

        // message loop
        let mut message = MSG::default();
        while GetMessageW(&mut message, HWND(0), 0, 0).into() {
            TranslateMessage(&mut message);
            DispatchMessageW(&mut message);
        }

        clock_thread_channel_sender.send(1).unwrap();
        let _ = clock_thread_join_handle.join();

        // dispose
        {
            let global_tcc_display_hm = GLOBAL_TCC_DISPLAY.lock().unwrap();
            for tcc_display in global_tcc_display_hm.values() {
                for tcc_panel in tcc_display.panels.values() {
                    if tcc_panel.background_init {
                        DeleteObject(tcc_panel.background_bm);
                        DeleteDC(tcc_panel.background_dc);    
                    }
                }
            }
        }

        for hfont in GLOBAL_HFONT_HM.lock().unwrap().iter() {
            DeleteObject(*hfont.1);
        }

        for hpen in GLOBAL_HPEN_HM.lock().unwrap().iter() {
            DeleteObject(*hpen.1);
        }

        DestroyMenu(*GLOBAL_MENU_HANDLE);

        // restore taskbar
        {
            let global_tcc_display_hm = GLOBAL_TCC_DISPLAY.lock().unwrap();
            for global_tcc_display in global_tcc_display_hm.values() {
                if global_tcc_display.taskbar_adjust_left != 0 || global_tcc_display.taskbar_adjust_right != 0 {
                    SetWindowPos(
                        global_tcc_display.taskbar_content_hwnd,
                        HWND_BOTTOM,
                        0,
                        0,
                        global_tcc_display.taskbar_rect.right - global_tcc_display.taskbar_rect.left,
                        global_tcc_display.taskbar_rect.bottom - global_tcc_display.taskbar_rect.top,
                        SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOSENDCHANGING
                    );
                }
            }
        }

        Ok(())
    }
}

fn create_window(wc: WNDCLASSW, ref_global_tcc_display: &mut TccDisplay, config_display: serde_json::Value) -> anyhow::Result<()> {
    let device_name = &ref_global_tcc_display.device_name;

    let class_name = convert_utf16_null("tcc_win11_window_class");
    let app_title = convert_utf16_null("tcc_win11");

    if config_display["panels"] == serde_json::Value::Null {
        return Err(anyhow!(format!("[nothing] config.txt displays > panels")));
    }
    if !config_display["panels"].is_array() {
        return Err(anyhow!(format!("[not array] config.txt displays > panels")));
    }
    let config_panels = config_display["panels"].as_array().unwrap();
    for i in 0..config_panels.len() {

        let config_panel = &config_panels[i];

        let mut tcc_panel = TccPanel::default();

        tcc_panel.id = ref_global_tcc_display.id.to_string() + "_" + &i.to_string();

        let key = "position";
        let config_value = &config_panel[key];
        if *config_value == serde_json::Value::Null {
            return Err(anyhow!(format!("[nothing] config.txt displays > panels[{i}] > {key}")));
        }
        if !config_value.is_string() {
            return Err(anyhow!(format!("[not string] config.txt displays > panels[{i}] > {key}")));
        }
        tcc_panel.position = match config_value.as_str() {
            Some("left") => { 0 }
            Some("center") => { 1 }
            Some("right") => { 2 }
            _ => {
                return Err(anyhow!(format!("[incorrect value] config.txt displays > panels[{i}] > {key} => \"left\" or \"center\" or \"right\"")));
            }
        };

        let key = "width";
        let config_value = &config_panel[key];
        if *config_value == serde_json::Value::Null {
            return Err(anyhow!(format!("[nothing] config.txt displays > panels[{i}] > {key}")));
        }
        if !config_value.is_i64() {
            return Err(anyhow!(format!("[not number] config.txt displays > panels[{i}] > {key}")));
        }
        let val = config_value.as_f64().unwrap() * ref_global_tcc_display.zoom;
        tcc_panel.width = val.ceil() as i32;

        let key = "left";
        let config_value = &config_panel[key];
        if *config_value == serde_json::Value::Null {
            return Err(anyhow!(format!("[nothing] config.txt displays > panels[{i}] > {key}")));
        }
        if !config_value.is_i64() {
            return Err(anyhow!(format!("[not number] config.txt displays > panels[{i}] > {key}")));
        }
        let val = config_value.as_f64().unwrap() * ref_global_tcc_display.zoom;
        tcc_panel.left = val.ceil() as i32;

        let key = "show_desktop_button_position";
        let config_value = &config_panel[key];
        if *config_value == serde_json::Value::Null {
            return Err(anyhow!(format!("[nothing] config.txt displays > panels[{i}] > {key}")));
        }
        if !config_value.is_string() {
            return Err(anyhow!(format!("[not string] config.txt displays > panels[{i}] > {key}")));
        }
        tcc_panel.show_desktop_button_position = match config_value.as_str() {
            Some("left") => { 0 }
            Some("center") => { 1 }
            Some("right") => { 2 }
            _ => { -1 }
        };

        tcc_panel.background_init = false;

        // create panel-window
        let mut window_x = 0;
        let mut window_y = 0;
        let window_height = ref_global_tcc_display.taskbar_rect.bottom - ref_global_tcc_display.taskbar_rect.top; 
        match tcc_panel.position {
            0 => {
                window_x = ref_global_tcc_display.taskbar_rect.left + tcc_panel.left;
                window_y = ref_global_tcc_display.display_rect.bottom - window_height;
            }
            1 => {
                window_x = (ref_global_tcc_display.taskbar_rect.left + ref_global_tcc_display.display_rect.right) / 2 - tcc_panel.width / 2 + tcc_panel.left;
                window_y = ref_global_tcc_display.display_rect.bottom - window_height;
            }
            2 => {
                window_x = ref_global_tcc_display.display_rect.right - tcc_panel.width + tcc_panel.left;
                window_y = ref_global_tcc_display.display_rect.bottom - window_height;
            }
            _ => {}
        }
        let nullptr: ::core::option::Option<*const ::core::ffi::c_void> = None;
        unsafe {
            tcc_panel.hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW, // | WS_EX_LAYERED,
                PCWSTR(class_name.as_ptr()),
                PCWSTR(app_title.as_ptr()),
                WS_VISIBLE | WS_POPUP,
                window_x,
                window_y,
                tcc_panel.width,
                window_height,
                None,
                None,
                wc.hInstance,
                nullptr,
            );
        }

        // labels
        if config_panel["labels"] == serde_json::Value::Null {
            return Err(anyhow!(format!("[nothing] config.txt displays > panels[i] > labels")));
        }
        if !config_panel["labels"].is_array() {
            return Err(anyhow!(format!("[not array] config.txt displays > panels[i] > labels")));
        }
        let config_labels = config_panel["labels"].as_array().unwrap();
        for j in 0..config_labels.len() {

            let config_label = &config_labels[j];

            let mut tcc_label = TccLabel::default();

            tcc_label.id = tcc_panel.id.to_string() + "_" + &j.to_string();

            let key = "timezone";
            let config_value = &config_label[key];
            if *config_value == serde_json::Value::Null {
                return Err(anyhow!(format!("[nothing] config.txt displays > panels[{i}] > labels[{j}] > {key}")));
            }
            if !config_value.is_string() {
                return Err(anyhow!(format!("[not string] config.txt displays > panels[{i}] > labels[{j}] > {key}")));
            }
            let mut config_string = config_value.as_str().unwrap();
            if config_string == "" {
                config_string = "UTC";
            }
            let tz_result: anyhow::Result<Tz, _> = config_string.parse();
            match tz_result {
                Err(_) => {
                    return Err(anyhow!(format!("[incorrect value] config.txt displays > panels[{i}] > labels[{j}] > {key}")));
                }
                anyhow::Result::Ok(_) => {
                    tcc_label.timezone = config_string.to_string();
                }
            }

            let key = "format";
            let config_value = &config_label[key];
            if *config_value == serde_json::Value::Null {
                return Err(anyhow!(format!("[nothing] config.txt displays > panels[{i}] > labels[{j}] > {key}")));
            }
            if !config_value.is_string() {
                return Err(anyhow!(format!("[not string] config.txt displays > panels[{i}] > labels[{j}] > {key}")));
            }
            tcc_label.format = config_label[key].as_str().unwrap().to_string();

            let key = "left";
            let config_value = &config_label[key];
            if *config_value == serde_json::Value::Null {
                return Err(anyhow!(format!("[nothing] config.txt displays > panels[{i}] > labels[{j}] > {key}")));
            }
            if !config_value.is_i64() {
                return Err(anyhow!(format!("[not number] config.txt displays > panels[{i}] > labels[{j}] > {key}")));
            }
            let val = config_value.as_f64().unwrap() * ref_global_tcc_display.zoom;
            tcc_label.left = val.ceil() as i32;

            let key = "top";
            let config_value = &config_label[key];
            if *config_value == serde_json::Value::Null {
                return Err(anyhow!(format!("[nothing] config.txt displays > panels[{i}] > labels[{j}] > {key}")));
            }
            if !config_value.is_i64() {
                return Err(anyhow!(format!("[not number] config.txt displays > panels[{i}] > labels[{j}] > {key}")));
            }
            let val = config_value.as_f64().unwrap() * ref_global_tcc_display.zoom;
            tcc_label.top = val.ceil() as i32;

            let key = "font_color";
            let config_value = &config_label[key];
            if *config_value == serde_json::Value::Null {
                return Err(anyhow!(format!("[nothing] config.txt displays > panels[{i}] > labels[{j}] > {key}")));
            }
            if !config_value.is_string() {
                return Err(anyhow!(format!("[not string] config.txt displays > panels[{i}] > labels[{j}] > {key}")));
            }
            let str_value = config_value.as_str().unwrap();
            let color_regex = Regex::new(r"[0-9a-fA-F]{6}").unwrap();
            if !color_regex.is_match(str_value) {
                return Err(anyhow!(format!("[incorrect value] config.txt displays > panels[{i}] > labels[{j}] > {key} => \"000000\" to \"FFFFFF\"")));
            }
            tcc_label.font_color = str_value.to_string();

            let key = "font_name";
            let config_value = &config_label[key];
            if *config_value == serde_json::Value::Null {
                return Err(anyhow!(format!("[nothing] config.txt displays > panels[{i}] > labels[{j}] > {key}")));
            }
            if !config_value.is_string() {
                return Err(anyhow!(format!("[not string] config.txt displays > panels[{i}] > labels[{j}] > {key}")));
            }
            tcc_label.font_name = config_label[key].as_str().unwrap().to_string();

            let key = "font_size";
            let config_value = &config_label[key];
            if *config_value == serde_json::Value::Null {
                return Err(anyhow!(format!("[nothing] config.txt displays > panels[{i}] > labels[{j}] > {key}")));
            }
            if !config_value.is_i64() {
                return Err(anyhow!(format!("[not number] config.txt displays > panels[{i}] > labels[{j}] > {key}")));
            }
            let val = config_value.as_f64().unwrap() * ref_global_tcc_display.zoom;
            tcc_label.font_size = val.ceil() as i32;

            let key = "font_bold";
            let config_value = &config_label[key];
            if *config_value == serde_json::Value::Null {
                return Err(anyhow!(format!("[nothing] config.txt displays > panels[{i}] > labels[{j}] > {key}")));
            }
            if !config_value.is_i64() {
                return Err(anyhow!(format!("[not number] config.txt displays > panels[{i}] > labels[{j}] > {key}")));
            }
            tcc_label.font_bold = config_value.as_i64().unwrap() as i32;

            let key = "font_italic";
            let config_value = &config_label[key];
            if *config_value == serde_json::Value::Null {
                return Err(anyhow!(format!("[nothing] config.txt displays > panels[{i}] > labels[{j}] > {key}")));
            }
            if !config_value.is_i64() {
                return Err(anyhow!(format!("[not number] config.txt displays > panels[{i}] > labels[{j}] > {key}")));
            }
            tcc_label.font_italic = config_value.as_i64().unwrap() as i32;

            tcc_panel.labels.push(tcc_label);

        }

        let mut tcc_panel_info = TccPanelInfo::default();
        tcc_panel_info.global_tcc_display_id = device_name.to_string();
        tcc_panel_info.panel_id = tcc_panel.id.clone();
        tcc_panel_info.show_desktop_button = tcc_panel.show_desktop_button_position.clone();
        tcc_panel_info.show_desktop_button_visible = false;
        set_global_tcc_panel_info(tcc_panel.hwnd.0, tcc_panel_info);

        ref_global_tcc_display.panels.insert(tcc_panel.id.clone(), tcc_panel);

    }

    Ok(())
}

unsafe extern "system" fn enumerate_callback_get_monitors_info(
    hmonitor: HMONITOR,
    _: HDC,
    _: *mut RECT,
    _: LPARAM,
) -> BOOL {
    let mut monitor_info_ex: MONITORINFOEXW = MONITORINFOEXW::default();
    monitor_info_ex.monitorInfo.cbSize = mem::size_of::<MONITORINFOEXW>() as u32;
    let monitor_info_ptr = <*mut _>::cast(&mut monitor_info_ex);
    let result = GetMonitorInfoW(hmonitor, monitor_info_ptr);
    if result == TRUE {
        let device_name = monitor_info_ex.szDevice.iter().map(|x| (char::from_u32(*x as u32)).unwrap()).collect::<String>();
        let mut tcc_display = TccDisplay::default();
        tcc_display.display_rect = monitor_info_ex.monitorInfo.rcMonitor;
        tcc_display.device_name = device_name.clone();

        let mut dpi_x = 0u32;
        let mut dpi_y = 0u32;
        let _ = GetDpiForMonitor(
            hmonitor,
            MDT_EFFECTIVE_DPI,
            &mut dpi_x,
            &mut dpi_y
        );
        tcc_display.zoom = dpi_x as f64 / 96.0;
        set_global_tcc_display(device_name, tcc_display);
    }
    TRUE
}

unsafe extern "system" fn enumerate_callback_get_taskbar_content_hwnd(hwnd: HWND, _: LPARAM) -> BOOL {
    let mut classname:Vec<u8> = vec![0; 100];
    GetClassNameA(hwnd, &mut classname);
    let a = String::from_utf8(classname).unwrap();
    let b = a.trim_end_matches(char::from(0));
    if b == "Windows.UI.Composition.DesktopWindowContentBridge" {
        set_global_hwnd(String::from("taskbar_content_hwnd"), hwnd);
        return FALSE;
    }
    return TRUE
}

extern "system" fn wndproc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match message as u32 {
            WM_PAINT => {

                let mut rect: RECT = RECT::default();
                GetClientRect(hwnd, &mut rect);

                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd,  &mut ps);

                let global_tcc_panel_info_hm = GLOBAL_TCC_PANEL_INFO.lock().unwrap();
                match global_tcc_panel_info_hm.get(&hwnd.0) {
                    None => {}
                    Some(global_tcc_panel_info) => {
                        let mut global_tcc_display_hm = GLOBAL_TCC_DISPLAY.lock().unwrap();
                        match global_tcc_display_hm.get_mut(&global_tcc_panel_info.global_tcc_display_id) {
                            None => {}
                            Some(global_tcc_display) => {
                                let hdc_mem: CreatedHDC = CreateCompatibleDC(hdc);
                                let hbm_mem: HBITMAP = CreateCompatibleBitmap(hdc, rect.right, rect.bottom);
                                SelectObject(hdc_mem, hbm_mem);

                                let tcc_panel = global_tcc_display.panels.get_mut(&global_tcc_panel_info.panel_id).unwrap();

                                // fill backcolor
                                let mut rect = RECT::default();
                                GetClientRect(hwnd, &mut rect);
                                if tcc_panel.background_init {
                                    BitBlt(
                                        hdc_mem,
                                        0,
                                        0,
                                        rect.right - rect.left,
                                        rect.bottom - rect.top,
                                        tcc_panel.background_dc,
                                        0,
                                        0,
                                        SRCCOPY
                                    );
                                }

                                // draw labels
                                let now_utc: DateTime<Utc> = *GLOBAL_UTC_NOW;
                                for i in 0..tcc_panel.labels.len() {
                                    let label = &mut tcc_panel.labels[i];
                                    label.draw(hdc_mem, now_utc);
                                }

                                // draw show desktop button
                                create_global_hpen("white_pen", "FFFFFF");
                                let hpen_hm = GLOBAL_HPEN_HM.lock().unwrap();
                                if global_tcc_panel_info.show_desktop_button_visible {
                                    let hpen = hpen_hm.get("white_pen").unwrap();
                                    SelectObject(hdc_mem, *hpen);
                                    match global_tcc_panel_info.show_desktop_button {
                                        0 => {
                                            MoveToEx(hdc_mem, 7, 2, None);
                                            LineTo(hdc_mem, 7, rect.bottom - 2);
                                        }
                                        1 => {
                                            MoveToEx(hdc_mem, (rect.right - rect.left) / 2, 0, None);
                                            LineTo(hdc_mem, (rect.right - rect.left) / 2, rect.bottom - 2);
                                        }
                                        2 => {
                                            MoveToEx(hdc_mem, rect.right - 7, 2, None);
                                            LineTo(hdc_mem, rect.right - 7, rect.bottom - 2);
                                        }
                                        _ => {}
                                    }
                                }

                                //
                                BitBlt(hdc, 0, 0, rect.right, rect.bottom, hdc_mem, 0, 0, SRCCOPY);
                                DeleteObject(hbm_mem);
                                DeleteDC(hdc_mem);
                            }
                        }
                    }
                }

                EndPaint(hwnd, &ps);

                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let mut global_tcc_panel_info_hm = GLOBAL_TCC_PANEL_INFO.lock().unwrap();
                match global_tcc_panel_info_hm.get_mut(&hwnd.0) {
                    Some(tcc_panel_info) => {
                        if tcc_panel_info.show_desktop_button_visible {
                            winput::press(Vk::LeftWin);
                            winput::send(Vk::D);
                            winput::release(Vk::LeftWin);
                        } else {
                            winput::press(Vk::LeftWin);
                            winput::send(Vk::N);
                            winput::release(Vk::LeftWin);
                        }
                    }
                    None => {
                        winput::press(Vk::LeftWin);
                        winput::send(Vk::N);
                        winput::release(Vk::LeftWin);
                    }
                }
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                let x = lparam.0 & 0xffff; //LOWORD(lparam);
        		let y = (lparam.0 >> 16) & 0xffff; //HIWORD(lparam);
                let mut po = POINT{
                    x: x as i32,
                    y: y as i32,
                };
                ClientToScreen(hwnd , &mut po);
                TrackPopupMenu(
                    *GLOBAL_MENU_HANDLE,
                    TPM_LEFTALIGN | TPM_BOTTOMALIGN,
                    po.x,
                    po.y,
                    0,
                    hwnd,
                    None
                );
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let mut window_refresh = false;
                {
                    let mut global_tcc_panel_info_hm = GLOBAL_TCC_PANEL_INFO.lock().unwrap();
                    match global_tcc_panel_info_hm.get_mut(&hwnd.0) {
                        Some(tcc_panel_info) => {
                            if tcc_panel_info.show_desktop_button >= 0 {
                                let mut rect: RECT = RECT::default();
                                GetClientRect(hwnd, &mut rect);
                                let x = lparam.0 & 0xffff;
                                match tcc_panel_info.show_desktop_button {
                                    0 => {
                                        if x >= 0 && x <= 15 {
                                            if !tcc_panel_info.show_desktop_button_visible {
                                                tcc_panel_info.show_desktop_button_visible = true;
                                                window_refresh = true;
                                            }
                                        } else {
                                            if tcc_panel_info.show_desktop_button_visible {
                                                tcc_panel_info.show_desktop_button_visible = false;
                                                window_refresh = true;
                                            }
                                        }
                                    }
                                    1 => {
                                        if x >= ((rect.right - rect.left) / 2 - 8) as isize  && x <= ((rect.right - rect.left) / 2 + 8) as isize {
                                            if !tcc_panel_info.show_desktop_button_visible {
                                                tcc_panel_info.show_desktop_button_visible = true;
                                                window_refresh = true;
                                            }
                                        } else {
                                            if tcc_panel_info.show_desktop_button_visible {
                                                tcc_panel_info.show_desktop_button_visible = false;
                                                window_refresh = true;
                                            }
                                        }
                                    }
                                    2 => {
                                        if x >= (rect.right - rect.left - 15) as isize  && x <= (rect.right - rect.left) as isize {
                                            if !tcc_panel_info.show_desktop_button_visible {
                                                tcc_panel_info.show_desktop_button_visible = true;
                                                window_refresh = true;
                                            }
                                        } else {
                                            if tcc_panel_info.show_desktop_button_visible {
                                                tcc_panel_info.show_desktop_button_visible = false;
                                                window_refresh = true;
                                            }
                                        }
                                    }
                                    _ => {
                                        if tcc_panel_info.show_desktop_button_visible {
                                            tcc_panel_info.show_desktop_button_visible = false;
                                            window_refresh = true;
                                        }
                                }
                                }
                                let mut tme = TRACKMOUSEEVENT::default();
                                tme.cbSize = mem::size_of::<TRACKMOUSEEVENT>() as u32;
                                tme.dwFlags = TME_LEAVE;
                                tme.hwndTrack = hwnd;
                                TrackMouseEvent(&mut tme);
                            }
                        }
                        None => {}
                    }
                }
                if window_refresh {
                    InvalidateRect(hwnd, None, true);
                    UpdateWindow(hwnd);
                }
                LRESULT(0)
            }
            675 => {
                // WM_MOUSELEAVE
                let mut global_tcc_panel_info_hm = GLOBAL_TCC_PANEL_INFO.lock().unwrap();
                match global_tcc_panel_info_hm.get_mut(&hwnd.0) {
                    Some(tcc_panel_info) => {
                        tcc_panel_info.show_desktop_button_visible = false;
                    }
                    None => {}
                }
                LRESULT(0)
            }
            WM_COMMAND => {
                if wparam.0 == 1 && lparam.0 == 0 {
                    if !GLOBAL_END {
                        GLOBAL_END = true;
                        PostQuitMessage(0);
                    }
                } else if wparam.0 == 2 && lparam.0 == 0 {
                    if !GLOBAL_END {
                        GLOBAL_END = true;
                        let pid = Win32::System::Threading::GetCurrentProcessId();
                        Win32::UI::Shell::ShellExecuteW(
                            None,
                            None,
                            PCWSTR(convert_utf16_null("tcc-win11.exe").as_ptr()),
                            PCWSTR(convert_utf16_null(&pid.to_string()).as_ptr()),
                            PCWSTR(convert_utf16_null("").as_ptr()),
                            SW_SHOWNORMAL
                        );
                        PostQuitMessage(0);
                    }
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                if !GLOBAL_END {
                    GLOBAL_END = true;
                    PostQuitMessage(0);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                if !GLOBAL_END {
                    GLOBAL_END = true;
                    PostQuitMessage(0);
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}
