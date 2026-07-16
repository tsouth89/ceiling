//! Windows access-control helpers for user-scoped local IPC and secret files.

use std::io;
use std::mem::{size_of, size_of_val};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACL, ACL_REVISION, AddAccessAllowedAce, DACL_SECURITY_INFORMATION,
    GetLengthSid, GetTokenInformation, InitializeAcl, InitializeSecurityDescriptor,
    PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SE_DACL_PROTECTED,
    SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR, SetFileSecurityW, SetSecurityDescriptorControl,
    SetSecurityDescriptorDacl, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
use windows::Win32::System::SystemServices::SECURITY_DESCRIPTOR_REVISION;
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::PCWSTR;

fn windows_error(context: &str, error: impl std::fmt::Debug) -> io::Error {
    io::Error::other(format!("{context}: {error:?}"))
}

/// Owns a protected DACL containing one full-control ACE for the current user.
///
/// The descriptor points into `acl_buffer`, so callers must keep this value alive
/// until the Win32 operation consuming the descriptor has returned.
pub struct CurrentUserOnlySecurityDescriptor {
    acl_buffer: Vec<usize>,
    descriptor: SECURITY_DESCRIPTOR,
}

/// Owns a Win32 `SECURITY_ATTRIBUTES` value while hiding the Windows crate type
/// from callers that only need to pass its raw pointer to an OS-backed API.
pub struct CurrentUserOnlySecurityAttributes<'a> {
    raw: SECURITY_ATTRIBUTES,
    _descriptor: std::marker::PhantomData<&'a mut CurrentUserOnlySecurityDescriptor>,
}

impl CurrentUserOnlySecurityAttributes<'_> {
    pub fn as_mut_ptr(&mut self) -> *mut core::ffi::c_void {
        (&mut self.raw as *mut SECURITY_ATTRIBUTES).cast()
    }
}

impl CurrentUserOnlySecurityDescriptor {
    pub fn new() -> io::Result<Self> {
        unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
                .map_err(|error| windows_error("OpenProcessToken failed", error))?;

            let result = Self::from_token(token);
            let _ = CloseHandle(token);
            result
        }
    }

    unsafe fn from_token(token: HANDLE) -> io::Result<Self> {
        let mut token_bytes = 0u32;
        let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut token_bytes) };
        if token_bytes < size_of::<TOKEN_USER>() as u32 {
            return Err(io::Error::other(
                "GetTokenInformation returned an invalid TOKEN_USER size",
            ));
        }

        // Allocate in machine words so TOKEN_USER and its trailing SID stay aligned.
        let token_words = (token_bytes as usize).div_ceil(size_of::<usize>());
        let mut token_buffer = vec![0usize; token_words];
        unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                Some(token_buffer.as_mut_ptr().cast()),
                token_bytes,
                &mut token_bytes,
            )
        }
        .map_err(|error| windows_error("GetTokenInformation failed", error))?;

        let token_user = unsafe { &*(token_buffer.as_ptr().cast::<TOKEN_USER>()) };
        let sid = token_user.User.Sid;
        let sid_bytes = unsafe { GetLengthSid(sid) } as usize;
        if sid_bytes == 0 {
            return Err(io::Error::other("GetLengthSid returned zero"));
        }

        let acl_bytes = size_of::<ACL>() + size_of::<ACCESS_ALLOWED_ACE>() + sid_bytes
            - size_of_val(&ACCESS_ALLOWED_ACE::default().SidStart);
        let acl_words = acl_bytes.div_ceil(size_of::<usize>());
        let mut acl_buffer = vec![0usize; acl_words];
        let acl = acl_buffer.as_mut_ptr().cast::<ACL>();
        unsafe {
            InitializeAcl(acl, acl_bytes as u32, ACL_REVISION)
                .map_err(|error| windows_error("InitializeAcl failed", error))?;
            AddAccessAllowedAce(acl, ACL_REVISION, FILE_ALL_ACCESS.0, sid)
                .map_err(|error| windows_error("AddAccessAllowedAce failed", error))?;
        }

        let mut descriptor = SECURITY_DESCRIPTOR::default();
        let descriptor_ptr =
            PSECURITY_DESCRIPTOR((&mut descriptor as *mut SECURITY_DESCRIPTOR).cast());
        unsafe {
            InitializeSecurityDescriptor(descriptor_ptr, SECURITY_DESCRIPTOR_REVISION)
                .map_err(|error| windows_error("InitializeSecurityDescriptor failed", error))?;
            SetSecurityDescriptorDacl(descriptor_ptr, true, Some(acl), false)
                .map_err(|error| windows_error("SetSecurityDescriptorDacl failed", error))?;
            SetSecurityDescriptorControl(descriptor_ptr, SE_DACL_PROTECTED, SE_DACL_PROTECTED)
                .map_err(|error| windows_error("SetSecurityDescriptorControl failed", error))?;
        }

        Ok(Self {
            acl_buffer,
            descriptor,
        })
    }

    fn descriptor_ptr(&mut self) -> PSECURITY_DESCRIPTOR {
        debug_assert!(!self.acl_buffer.is_empty());
        PSECURITY_DESCRIPTOR((&mut self.descriptor as *mut SECURITY_DESCRIPTOR).cast())
    }

    /// Build security attributes suitable for CreateNamedPipeW and similar APIs.
    pub fn security_attributes(&mut self) -> CurrentUserOnlySecurityAttributes<'_> {
        CurrentUserOnlySecurityAttributes {
            raw: SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: self.descriptor_ptr().0,
                bInheritHandle: false.into(),
            },
            _descriptor: std::marker::PhantomData,
        }
    }
}

/// Replace a file or directory DACL with a protected current-user-only ACL.
pub fn restrict_path_to_current_user(path: &Path) -> io::Result<()> {
    let mut security = CurrentUserOnlySecurityDescriptor::new()?;
    let wide_path: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        SetFileSecurityW(
            PCWSTR(wide_path.as_ptr()),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            security.descriptor_ptr(),
        )
        .ok()
        .map_err(|error| windows_error("SetFileSecurityW failed", error))
    }
}
