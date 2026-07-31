/**
 * metadump.c - Stealth in-process dmabuf metadata dumper
 *
 * Loaded via LSPosed/Zygisk into com.pinkcore.darakugear process.
 * Constructor scans /proc/self/maps for dmabuf:METADATA and dumps.
 *
 * STEALTH FEATURES:
 * - Writes to app's private data dir (not /data/local/tmp)
 * - Tries multiple output paths, preferring app-internal storage
 * - No logging to logcat (avoids detection)
 * - Fail-silent: returns cleanly on any error
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/stat.h>
#include <errno.h>

#define TARGET_PROCESS "com.pinkcore.darakugear"

/* Preferred output paths in order of stealthiness */
static const char *OUTPUT_PATHS[] = {
    "/data/data/com.pinkcore.darakugear/files/metadata_dump.bin",
    "/data/user/0/com.pinkcore.darakugear/files/metadata_dump.bin",
    "/sdcard/Android/data/com.pinkcore.darakugear/files/metadata_dump.bin",
    "/data/local/tmp/metadata_dump.bin",  /* fallback */
    NULL
};

/* Marker confirms success */
static const char *MARKER_PATH =
    "/data/data/com.pinkcore.darakugear/files/metadata_dump.done";

__attribute__((constructor))
void dump_metadata(void) {
    char cmdline[256] = {0};
    FILE *maps;
    char line[512];
    int out_fd = -1;
    unsigned long total = 0;
    const char *out_path = NULL;

    /* Check if we're in the target process */
    int cmdfd = open("/proc/self/cmdline", O_RDONLY);
    if (cmdfd < 0) return;
    ssize_t n = read(cmdfd, cmdline, sizeof(cmdline) - 1);
    close(cmdfd);
    if (n <= 0) return;

    if (!strstr(cmdline, TARGET_PROCESS)) return;

    /* Try output paths in order */
    for (int i = 0; OUTPUT_PATHS[i] != NULL; i++) {
        /* Ensure parent directory exists */
        char dir[256];
        strncpy(dir, OUTPUT_PATHS[i], sizeof(dir) - 1);
        dir[sizeof(dir) - 1] = '\0';
        char *slash = strrchr(dir, '/');
        if (slash) {
            *slash = '\0';
            mkdir(dir, 0755);  /* ignore error, might already exist */
        }

        out_fd = open(OUTPUT_PATHS[i],
                      O_WRONLY | O_CREAT | O_TRUNC, 0600);
        if (out_fd >= 0) {
            out_path = OUTPUT_PATHS[i];
            break;
        }
    }

    if (out_fd < 0) return;  /* all output paths failed, silent exit */

    /* Open process memory map */
    maps = fopen("/proc/self/maps", "r");
    if (!maps) { close(out_fd); return; }

    while (fgets(line, sizeof(line), maps)) {
        unsigned long start, end;
        char perms[8] = {0};
        char path[256] = {0};
        int fields;

        fields = sscanf(line, "%lx-%lx %4s %*x %*s %*d %255s",
                       &start, &end, perms, path);

        if (fields < 2) continue;

        /* Match dmabuf:METADATA regions */
        if (fields >= 3 && strstr(path, "dmabuf:METADATA")) {
            size_t size = end - start;

            /* Read in chunks to avoid SIGSEGV on partial mappings */
            const char *ptr = (const char *)(unsigned long)start;
            size_t remaining = size;
            while (remaining > 0) {
                size_t chunk = remaining > 4096 ? 4096 : remaining;
                ssize_t written = write(out_fd, ptr, chunk);
                if (written <= 0) break;
                ptr += written;
                remaining -= written;
                total += written;
            }
        }
    }

    fclose(maps);
    close(out_fd);

    /* Write done marker (in app private dir) */
    if (total > 0) {
        char marker_dir[256];
        strncpy(marker_dir, MARKER_PATH, sizeof(marker_dir) - 1);
        char *slash = strrchr(marker_dir, '/');
        if (slash) {
            *slash = '\0';
            mkdir(marker_dir, 0755);
        }

        int marker = open(MARKER_PATH,
                         O_WRONLY | O_CREAT | O_TRUNC, 0600);
        if (marker >= 0) {
            char buf[128];
            int len = snprintf(buf, sizeof(buf),
                              "dumped %lu bytes to %s\n", total, out_path);
            write(marker, buf, len);
            close(marker);
        }
    }
}
