/* Minimal Win32 loader for the `version.dll` proxy.
 *
 * The proxy has exactly one job: get `vapor_forge_host_windows.dll` mapped into
 * Steam before the client initialises. Every export of the real module is a PE
 * forwarder (see version.def), resolved by the Windows loader, so no version.dll
 * call ever reaches this file. This code only runs once, on process attach.
 *
 * The proxy must never make the client worse than it was without us, so a host
 * DLL that is missing or fails to load is not an error condition: Steam starts
 * normally and simply runs uninstrumented. The failure is reported to the
 * debugger rather than swallowed, because "injected but silent" is the one
 * outcome nobody can diagnose from the outside.
 */
#define WIN32_LEAN_AND_MEAN
#include <windows.h>

/* Host DLL installed next to the proxy. Deliberately a bare name: the proxy
 * lives in Steam's own directory, which is the application directory, so the
 * default search order finds the host there. */
#define HOST_DLL L"vapor_forge_host_windows.dll"
#define HOST_MISSING_MESSAGE L"vapor-forge proxy: host DLL not loaded\n"

/* Held for the whole process lifetime and never freed: the host owns detours,
 * threads and caches that must outlive any unload, and the Windows loader does
 * not run a DLL's exports again after a reload would have unmapped it. */
static HMODULE g_host_module;

BOOL WINAPI DllMain(HINSTANCE instance, DWORD reason, LPVOID reserved)
{
    (void)reserved;

    if (reason == DLL_PROCESS_ATTACH) {
        /* No exported function of ours runs on a thread, so thread-level
         * notifications are pure overhead. */
        DisableThreadLibraryCalls(instance);

        g_host_module = LoadLibraryW(HOST_DLL);
        if (g_host_module == NULL) {
            OutputDebugStringW(HOST_MISSING_MESSAGE);
        }
    }

    /* Always TRUE: a proxy that refuses to load would take Steam down with it,
     * and DllMain failures are indistinguishable from a broken install. */
    return TRUE;
}
