//! Download HTTPS con WinHTTP, il client di Windows: proxy e certificati di sistema,
//! redirect seguiti in automatico, niente librerie in più nell'exe.

use std::ffi::c_void;

use windows::Win32::Networking::WinHttp::*;
use windows::core::{HSTRING, PCWSTR, w};

use crate::{tr, trf};

/// Un download più grande di così non è un installer di Wheelor.
const MAX_SIZE: usize = 64 * 1024 * 1024;

/// Un handle di WinHTTP, chiuso quando esce di scope.
struct Handle(*mut c_void);

impl Handle {
    /// WinHTTP restituisce un puntatore nullo in caso di errore.
    fn new(raw: *mut c_void) -> Result<Self, String> {
        if raw.is_null() { Err(error(windows::core::Error::from_thread())) } else { Ok(Self(raw)) }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}

/// Scarica `url` (solo https) e ne restituisce il contenuto.
/// `progress(scaricati, totali)` viene chiamata dopo ogni blocco; `totali` è 0 se il server non lo dice.
pub fn get(url: &str, mut progress: impl FnMut(u64, u64)) -> Result<Vec<u8>, String> {
    let rest = url.strip_prefix("https://").ok_or_else(|| trf!("indirizzo non sicuro: {url}", "insecure address: {url}"))?;
    let (host, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let path = if path.is_empty() { "/" } else { path };

    unsafe {
        let agent = HSTRING::from(format!("Wheelor/{}", env!("CARGO_PKG_VERSION")));
        let session = Handle::new(WinHttpOpen(
            &agent,
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ))?;
        // Millisecondi per: trovare il server, collegarsi, inviare, ricevere ogni blocco.
        let _ = WinHttpSetTimeouts(session.0, 10_000, 10_000, 10_000, 30_000);
        let connection = Handle::new(WinHttpConnect(session.0, &HSTRING::from(host), INTERNET_DEFAULT_HTTPS_PORT, 0))?;
        let request = Handle::new(WinHttpOpenRequest(
            connection.0,
            w!("GET"),
            &HSTRING::from(path),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ))?;
        WinHttpSendRequest(request.0, None, None, 0, 0, 0).map_err(error)?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(error)?;

        match header_number(&request, WINHTTP_QUERY_STATUS_CODE) {
            Some(200) => {}
            Some(404) => return Err(tr!("file non trovato sul server", "file not found on the server").into()),
            Some(status) => return Err(trf!("il server ha risposto {status}", "the server answered {status}")),
            None => return Err(tr!("risposta del server non valida", "invalid answer from the server").into()),
        }
        let total = header_number(&request, WINHTTP_QUERY_CONTENT_LENGTH).unwrap_or(0) as usize;
        if total > MAX_SIZE {
            return Err(tr!("file troppo grande", "file too large").into());
        }

        let mut data = Vec::with_capacity(total);
        loop {
            let mut available = 0u32;
            WinHttpQueryDataAvailable(request.0, &mut available).map_err(error)?;
            if available == 0 {
                break; // fine del contenuto
            }
            let start = data.len();
            if start + available as usize > MAX_SIZE {
                return Err(tr!("file troppo grande", "file too large").into());
            }
            data.resize(start + available as usize, 0);
            let mut read = 0u32;
            WinHttpReadData(request.0, data[start..].as_mut_ptr().cast(), available, &mut read).map_err(error)?;
            data.truncate(start + read as usize);
            progress(data.len() as u64, total as u64);
        }
        if total > 0 && data.len() != total {
            return Err(tr!("download interrotto", "download interrupted").into());
        }
        Ok(data)
    }
}

/// Un'intestazione numerica della risposta (codice di stato, lunghezza…).
fn header_number(request: &Handle, info: u32) -> Option<u32> {
    let mut value = 0u32;
    let mut size = size_of::<u32>() as u32;
    unsafe {
        WinHttpQueryHeaders(
            request.0,
            info | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut value as *mut u32 as *mut c_void),
            &mut size,
            std::ptr::null_mut(),
        )
    }
    .ok()?;
    Some(value)
}

/// Messaggio comprensibile per gli errori di WinHTTP più comuni (i loro testi non stanno
/// nelle tabelle di sistema, quindi `message()` sarebbe vuoto).
fn error(e: windows::core::Error) -> String {
    // HRESULT_FROM_WIN32: il codice di Windows sta nei 16 bit bassi.
    match e.code().0 as u32 & 0xFFFF {
        12002 => tr!("il server non risponde", "the server doesn't answer").into(), // TIMEOUT
        12007 => tr!("nessuna connessione a internet", "no internet connection").into(), // NAME_NOT_RESOLVED
        12029 | 12030 => tr!("connessione non riuscita", "connection failed").into(), // CANNOT_CONNECT, CONNECTION_ERROR
        12175 | 12037 | 12038 | 12044 | 12045 => tr!("connessione sicura non riuscita", "secure connection failed").into(), // certificati
        code => trf!("errore di rete {code}", "network error {code}"),
    }
}
