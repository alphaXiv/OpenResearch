#include <sys/types.h>
#include <sys/sysctl.h>
#include <errno.h>

int orx_process_uid(pid_t pid, uid_t *uid) {
    int mib[] = {CTL_KERN, KERN_PROC, KERN_PROC_PID, pid};
    struct kinfo_proc info;
    size_t size = sizeof(info);
    if (sysctl(mib, 4, &info, &size, NULL, 0) == -1) return -1;
    if (size != sizeof(info)) {
        errno = size == 0 ? ESRCH : EIO;
        return -1;
    }
    /* Match proc_bsdinfo.pbi_uid, including setuid processes. */
    *uid = info.kp_eproc.e_ucred.cr_uid;
    return 0;
}
