//! User-selected call detection. Only provider/window identifiers leave the detector.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Provider {
    Teams,
    Zoom,
    Webex,
    GoogleMeet,
}

impl Provider {
    /// Every provider in the order the settings list them.
    pub const ALL: [Provider; 4] = [
        Provider::Teams,
        Provider::Zoom,
        Provider::Webex,
        Provider::GoogleMeet,
    ];

    /// The name shown to the user.
    pub fn label(self) -> &'static str {
        match self {
            Provider::Teams => "Microsoft Teams",
            Provider::Zoom => "Zoom",
            Provider::Webex => "Webex",
            Provider::GoogleMeet => "Google Meet",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallWindow {
    pub(crate) provider: Provider,
    pub(crate) window: String,
    pub(crate) key: String,
    // Only the core compares room identity. Never expose the address or its digest.
    #[serde(skip)]
    pub(crate) room_identity: Option<[u8; 32]>,
    #[serde(skip)]
    pub(crate) desktop_teams: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallScan {
    pub(crate) supported: bool,
    pub(crate) complete: bool,
    pub(crate) active: Vec<CallWindow>,
    pub(crate) ended: Vec<CallWindow>,
    pub(crate) open_windows: Vec<String>,
}

#[tauri::command]
pub async fn get_call_detection_status(
    providers: Vec<Provider>,
    watched: Vec<String>,
) -> Result<CallScan, String> {
    scan(providers, watched).await
}

/// One scan of the selected providers, run off the async workers. Shared by the
/// settings check and the core's call monitor.
pub(crate) async fn scan(
    providers: Vec<Provider>,
    watched: Vec<String>,
) -> Result<CallScan, String> {
    #[cfg(windows)]
    {
        tauri::async_runtime::spawn_blocking(move || native::scan(&providers, &watched))
            .await
            .map_err(|_| "Could not check meeting windows.".to_string())
    }
    #[cfg(not(windows))]
    {
        let _ = (providers, watched);
        Ok(CallScan::default())
    }
}

fn desktop_provider(exe: &str) -> Option<Provider> {
    match exe {
        "teams.exe" | "ms-teams.exe" | "msteams.exe" => Some(Provider::Teams),
        "zoom.exe" => Some(Provider::Zoom),
        "webex.exe" | "ciscocollabhost.exe" | "atmgr.exe" => Some(Provider::Webex),
        _ => None,
    }
}

pub(crate) fn teams_room(url: &url::Url) -> Option<[u8; 32]> {
    use sha2::{Digest, Sha256};
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || !matches!(
            url.host_str(),
            Some("teams.microsoft.com" | "teams.cloud.microsoft" | "teams.live.com")
        )
    {
        return None;
    }
    let path = url.path().trim_end_matches('/');
    let segments: Vec<_> = path.split('/').collect();
    let canonical = match segments.as_slice() {
        ["", "meet", id] if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) => {
            path.to_owned()
        }
        ["", "l", "meetup-join", thread, "0"] if !thread.is_empty() => {
            let mut bytes = thread.bytes();
            let mut decoded = String::with_capacity(thread.len());
            while let Some(byte) = bytes.next() {
                let value = if byte == b'%' {
                    let high = char::from(bytes.next()?).to_digit(16)?;
                    let low = char::from(bytes.next()?).to_digit(16)?;
                    (high * 16 + low) as u8
                } else {
                    byte
                };
                if !value.is_ascii_alphanumeric() && !b"-_.:@".contains(&value) {
                    return None;
                }
                decoded.push(char::from(value));
            }
            format!("/l/meetup-join/{decoded}/0")
        }
        _ => return None,
    };
    Some(Sha256::digest(format!("{}{canonical}", url.host_str()?)).into())
}

fn browser_surface(address: &str) -> Option<(Provider, String, Option<[u8; 32]>)> {
    let address = address.trim();
    let full = if address.contains("://") {
        address.to_owned()
    } else {
        format!("https://{address}")
    };
    let url = url::Url::parse(&full).ok()?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    match url.host_str()? {
        "meet.google.com" => Some((
            Provider::GoogleMeet,
            url.path().strip_prefix('/')?.to_owned(),
            Some(meet_room(&url)?),
        )),
        "teams.microsoft.com" | "teams.cloud.microsoft" | "teams.live.com" => {
            Some((Provider::Teams, url.path().into(), teams_room(&url)))
        }
        _ => None,
    }
}

pub(crate) fn meet_room(url: &url::Url) -> Option<[u8; 32]> {
    use sha2::{Digest, Sha256};
    if url.scheme() != "https"
        || url.host_str() != Some("meet.google.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    let code = url.path().strip_prefix('/')?;
    let parts = code.split('-');
    (parts.clone().map(str::len).eq([3, 4, 3])
        && parts
            .flat_map(str::bytes)
            .all(|byte| byte.is_ascii_lowercase()))
    .then(|| Sha256::digest(format!("meet.google.com/{code}")).into())
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::{
        collections::HashMap,
        hash::{DefaultHasher, Hash, Hasher},
        time::{Duration, Instant},
    };
    use windows::{
        core::{Interface, VARIANT},
        Win32::{
            Foundation::{BOOL, HANDLE, HWND, LPARAM},
            System::{
                Com::{
                    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
                    COINIT_MULTITHREADED,
                },
                StationsAndDesktops::{
                    CloseDesktop, GetUserObjectInformationW, OpenInputDesktop,
                    DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS, UOI_NAME,
                },
            },
            UI::{
                Accessibility::*,
                WindowsAndMessaging::{
                    EnumWindows, GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
                    IsWindowVisible,
                },
            },
        },
    };

    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }

    fn interactive_desktop() -> bool {
        unsafe {
            let Ok(desktop) =
                OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS)
            else {
                return false;
            };
            let mut name = [0u16; 64];
            let ok = GetUserObjectInformationW(
                HANDLE(desktop.0),
                UOI_NAME,
                Some(name.as_mut_ptr().cast()),
                128,
                None,
            )
            .is_ok();
            let _ = CloseDesktop(desktop);
            ok && GetForegroundWindow().0 != 0
                && String::from_utf16_lossy(
                    &name[..name.iter().position(|v| *v == 0).unwrap_or(name.len())],
                )
                .eq_ignore_ascii_case("default")
        }
    }

    struct Windows {
        processes: HashMap<u32, String>,
        windows: Vec<(HWND, u32)>,
        watched: std::collections::HashSet<(u32, isize)>,
    }
    unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> BOOL {
        let list = &mut *(data.0 as *mut Windows);
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if list.processes.contains_key(&pid) || list.watched.contains(&(pid, hwnd.0)) {
            list.windows.push((hwnd, pid));
        }
        BOOL::from(list.windows.len() < 2048)
    }

    fn any_text(
        api: &IUIAutomation,
        property: UIA_PROPERTY_ID,
        values: &[&str],
    ) -> Option<IUIAutomationCondition> {
        let mut condition: Option<IUIAutomationCondition> = None;
        for value in values {
            let next =
                unsafe { api.CreatePropertyCondition(property, &VARIANT::from(*value)) }.ok()?;
            condition = Some(match condition {
                Some(previous) => unsafe { api.CreateOrCondition(&previous, &next) }.ok()?,
                None => next,
            });
        }
        condition
    }

    fn address(api: &IUIAutomation, root: &IUIAutomationElement) -> Option<String> {
        // Limit reads to the browser's address control. Never enumerate form values.
        let class = any_text(api, UIA_ClassNamePropertyId, &["OmniboxViewViews"])?;
        let id = any_text(
            api,
            UIA_AutomationIdPropertyId,
            &["addressEditBox", "urlbar-input"],
        )?;
        let name = any_text(
            api,
            UIA_NamePropertyId,
            &[
                "Address and search bar",
                "Adress- und Suchleiste",
                "Address bar",
            ],
        )?;
        unsafe {
            let location = api.CreateOrCondition(&class, &id).ok()?;
            let location = api.CreateOrCondition(&location, &name).ok()?;
            let edit = api
                .CreatePropertyCondition(
                    UIA_ControlTypePropertyId,
                    &VARIANT::from(UIA_EditControlTypeId.0),
                )
                .ok()?;
            let location = api.CreateAndCondition(&edit, &location).ok()?;
            let element = root.FindFirst(TreeScope_Descendants, &location).ok()?;
            // A page can label a form control like the browser address bar.
            // Establish its ancestry before reading any ValuePattern.
            let walker = api.RawViewWalker().ok()?;
            let mut node = element.clone();
            let mut trusted = false;
            for _ in 0..32 {
                if node.CurrentControlType().ok()? == UIA_DocumentControlTypeId {
                    return None;
                }
                if api.CompareElements(&node, root).ok()?.as_bool() {
                    trusted = true;
                    break;
                }
                node = walker.GetParentElement(&node).ok()?;
            }
            if !trusted {
                return None;
            }
            let pattern = element
                .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                .ok()?;
            Some(pattern.CurrentValue().ok()?.to_string())
        }
    }

    fn button(api: &IUIAutomation, root: &IUIAutomationElement, names: &[&str]) -> bool {
        let find = || -> Option<()> {
            let name = any_text(api, UIA_NamePropertyId, names)?;
            unsafe {
                let kind = api
                    .CreatePropertyCondition(
                        UIA_ControlTypePropertyId,
                        &VARIANT::from(UIA_ButtonControlTypeId.0),
                    )
                    .ok()?;
                let condition = api.CreateAndCondition(&name, &kind).ok()?;
                let control = root.FindFirst(TreeScope_Descendants, &condition).ok()?;
                (control.CurrentIsEnabled().ok()?.as_bool()
                    && !control.CurrentIsOffscreen().ok()?.as_bool())
                .then_some(())
            }
        };
        find().is_some()
    }

    pub(super) fn scan(providers: &[Provider], watched: &[String]) -> CallScan {
        let mut scan = CallScan {
            supported: true,
            ..Default::default()
        };
        if providers.is_empty() {
            return scan;
        }
        // A locked or inaccessible desktop is an unknown observation.
        if !interactive_desktop() || unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_err()
        {
            return scan;
        }
        let _apartment = Apartment;
        let Ok(api) = (unsafe {
            CoCreateInstance::<_, IUIAutomation>(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
        }) else {
            return scan;
        };
        let Ok(timed) = api.cast::<IUIAutomation2>() else {
            return scan;
        };
        if unsafe {
            timed
                .SetConnectionTimeout(300)
                .and_then(|_| timed.SetTransactionTimeout(400))
        }
        .is_err()
        {
            return scan;
        }
        let mut system = sysinfo::System::new();
        system.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::All,
            true,
            sysinfo::ProcessRefreshKind::new(),
        );
        if system.processes().is_empty() {
            return scan;
        }
        let processes = system
            .processes()
            .iter()
            .filter_map(|(pid, p)| {
                let exe = p.name().to_string_lossy().to_ascii_lowercase();
                let browser = matches!(
                    exe.as_str(),
                    "msedge.exe" | "chrome.exe" | "firefox.exe" | "brave.exe"
                );
                let relevant = desktop_provider(&exe).is_some_and(|p| providers.contains(&p))
                    || (browser
                        && (providers.contains(&Provider::Teams)
                            || providers.contains(&Provider::GoogleMeet)));
                relevant.then_some((pid.as_u32(), exe))
            })
            .collect();
        let watched = watched
            .iter()
            .filter_map(|key| {
                let (pid, hwnd) = key.split_once('-')?;
                Some((
                    pid.parse().ok()?,
                    usize::from_str_radix(hwnd, 16).ok()? as isize,
                ))
            })
            .collect();
        let mut windows = Windows {
            processes,
            windows: vec![],
            watched,
        };
        scan.complete =
            unsafe { EnumWindows(Some(collect), LPARAM(&mut windows as *mut _ as isize)) }.is_ok();
        let deadline = Instant::now() + Duration::from_secs(4);
        for (hwnd, pid) in windows.windows {
            let window = format!("{pid}-{:x}", hwnd.0 as usize);
            scan.open_windows.push(window.clone());
            let Some(exe) = windows.processes.get(&pid) else {
                continue;
            };
            if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
                continue;
            }
            let mut title = [0u16; 512];
            let count = unsafe { GetWindowTextW(hwnd, &mut title) };
            if count <= 0 {
                continue;
            }
            let title = String::from_utf16_lossy(&title[..count as usize]).to_lowercase();
            let desktop = desktop_provider(exe);
            if desktop.is_none() && !title.contains("meet") && !title.contains("teams") {
                continue;
            }
            if Instant::now() >= deadline {
                scan.complete = false;
                continue;
            }
            let Ok(root) = (unsafe { api.ElementFromHandle(hwnd) }) else {
                continue;
            };
            let (provider, room, room_identity) = match desktop {
                Some(p) => (p, String::new(), None),
                None => {
                    let Some(surface) = address(&api, &root).and_then(|url| browser_surface(&url))
                    else {
                        continue;
                    };
                    surface
                }
            };
            if !providers.contains(&provider) {
                continue;
            }
            let mut hash = DefaultHasher::new();
            room.hash(&mut hash);
            let key = format!("{provider:?}-{window}-{:x}", hash.finish());
            let call = CallWindow {
                provider,
                window,
                key,
                room_identity,
                desktop_teams: desktop == Some(Provider::Teams),
            };
            let names: &[&str] = match provider {
                Provider::Zoom
                    if title == "zoom meeting" || title.starts_with("zoom meeting -") =>
                {
                    &[
                        "Leave",
                        "End",
                        "Leave Meeting",
                        "End Meeting",
                        "Verlassen",
                        "Beenden",
                        "Meeting verlassen",
                    ]
                }
                Provider::Teams => &[
                    "Leave call",
                    "Hang up",
                    "Leave meeting",
                    "Leave (Ctrl+Shift+H)",
                    "Auflegen",
                    "Anruf verlassen",
                    "Besprechung verlassen",
                ],
                Provider::GoogleMeet => &[
                    "Leave call",
                    "Leave call (Ctrl+Alt+H)",
                    "Anruf verlassen",
                    "Anruf beenden",
                ],
                _ => &[
                    "Leave meeting",
                    "End meeting",
                    "End meeting for all",
                    "Leave meeting or End meeting for all",
                    "Meeting verlassen",
                    "Meeting beenden",
                ],
            };
            let in_call = button(&api, &root, names)
                || (provider == Provider::Teams
                    && button(&api, &root, &["Leave", "Verlassen"])
                    && button(
                        &api,
                        &root,
                        &[
                            // Observed in the Teams meeting window on Windows: a bare
                            // "Leave" button next to "Mute mic" / "Unmute mic".
                            "Mute mic",
                            "Unmute mic",
                            "Mute",
                            "Unmute",
                            "Mute microphone",
                            "Unmute microphone",
                            "Stummschalten",
                            "Stummschaltung aufheben",
                        ],
                    ));
            if in_call {
                scan.active.push(call);
            } else if button(
                &api,
                &root,
                &[
                    "Rejoin",
                    "Rejoin meeting",
                    "Erneut teilnehmen",
                    "Wieder teilnehmen",
                ],
            ) {
                scan.ended.push(call);
            }
        }
        scan.complete &= interactive_desktop();
        scan
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_a_meeting_path_on_a_trusted_teams_origin_can_identify_a_room() {
        let room = |value: &str| teams_room(&url::Url::parse(value).unwrap());
        let meeting = room("https://teams.microsoft.com/meet/123456?p=synthetic");
        assert!(meeting.is_some());
        assert_eq!(meeting, room("https://teams.microsoft.com/meet/123456"));
        assert_ne!(meeting, room("https://teams.live.com/meet/123456"));
        assert_ne!(meeting, room("https://teams.microsoft.com/meet/789"));
        assert!(room(
            "https://teams.cloud.microsoft/l/meetup-join/19%3ameeting_example%40thread.v2/0"
        )
        .is_some());
        assert_eq!(
            room("https://teams.cloud.microsoft/l/meetup-join/19%3Ameeting_example%40thread.v2/0"),
            room("https://teams.cloud.microsoft/l/meetup-join/19:meeting_example@thread.v2/0?context=example")
        );
        for address in [
            "http://teams.microsoft.com/meet/123",
            "https://teams.microsoft.com.example.org/meet/123",
            "https://user@teams.microsoft.com/meet/123",
            "https://teams.microsoft.com:444/meet/123",
            "https://teams.microsoft.com/v2/",
            "https://teams.microsoft.com/meet/123/extra",
            "https://teams.microsoft.com/l/meetup-join/room%2Felsewhere/0",
            "https://teams.microsoft.com/l/meetup-join/room%zz/0",
        ] {
            assert!(room(address).is_none());
        }
    }

    #[test]
    fn browser_meeting_requires_a_provider_origin_and_meet_room() {
        assert!(matches!(
            browser_surface("meet.google.com/abc-defg-hij"),
            Some((Provider::GoogleMeet, _, _))
        ));
        for url in [
            "https://meet.google.com/landing",
            "https://meet.google.com.example.org/abc-defg-hij",
            "https://meet.google.com@example.org/abc-defg-hij",
            "http://meet.google.com/abc-defg-hij",
            "https://example.org/teams.microsoft.com",
        ] {
            assert!(browser_surface(url).is_none());
        }
        assert_eq!(desktop_provider("zoom-notes.exe"), None);
        assert_eq!(
            desktop_provider("ciscocollabhost.exe"),
            Some(Provider::Webex)
        );
    }
}
