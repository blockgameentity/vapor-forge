//! The loadable host: entry point, worker, and the report it leaves behind.
//!
//! The split is the one the loader lock forces:
//!
//! * [`dll`] (Windows only) is `DllMain`. It does two things and nothing else:
//!   `DLL_PROCESS_ATTACH` disables thread notifications and starts one worker
//!   thread through a raw `CreateThread`; `DLL_PROCESS_DETACH` sets a stop flag.
//!   No allocation, no file I/O, no lock is taken there.
//! * [`worker`] is the whole host flow, written against three traits
//!   ([`worker::HostEnv`], the installer factory, [`worker::StateSink`]) so the
//!   identical code path runs in a live `steamclient64.dll` and in a Linux unit
//!   test with fakes. It never panics: a failure disables its own capability
//!   through [`crate::capability::Gate`], and a failing module costs only that
//!   module.
//! * [`windows`] (Windows only) supplies the real environment: the host module's
//!   own path, the config path, the corpus directory, `<steam>/depotcache`, and
//!   the loaded Steam modules.
//! * [`paths`] resolves every path, with the environment passed in so the rules
//!   are testable.
//! * [`log`] installs the one small `tracing` sink: without it every event is
//!   dropped and a report that says "did not resolve" gives no way to see why.
//! * [`report`] renders and writes the bundle the report-bundle rule requires.

pub mod log;
pub mod paths;
pub mod report;
pub mod worker;

#[cfg(test)]
mod tests;

#[cfg(windows)]
pub mod dll;
#[cfg(windows)]
pub mod windows;
