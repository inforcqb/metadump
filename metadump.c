/**
 * metadump.c - Polling dmabuf metadata dumper for DarakuGear
 *
 * Constructor auto-runs on dlopen. Since dmabuf:METADATA may not be
 * mapped yet at Application.attach(), we poll /proc/self/maps for up
 * to 30 seconds waiting for the region to appear.
 *
 * Uses LOGI/LOGE macros for logcat visibility.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/stat.h>
#include <errno.h>
#include <android/log.h>

#define TAG "metadump"
#define LOGI(...) __android_log_print(ANDROID_LOG_INFO, TAG, __VA_ARGS__)
#define LOGE(...) __android_log_print(ANDROID_LOG_ERROR, TAG, __VA_ARGS__)

#define TARGET_PROCESS "com.pinkcore.darakugear"
#define POLL_MAX_RETRIES 60
#define POLL_INTERVAL_MS 500000

/* Preferred output paths in order */
static const char *OUTPUT_PATHS[] = {
    "/data/data/com.pinkcore.darakugear/files/metadata_dump.bin",
    "/sdcard/Android/data/com.pinkcore.darakugear/files/metadata_dump.bin",
    "/data/local/tmp/metadata_dump.bin",
    NULL
};

static const char *MARKER_PATH =
    "/data/local/tmp/metadata_dump.done";

__attribute__((constructor))
void dump_metadata(void) {
    char cmdline[256] = {0};
    int cmdfd, out_fd;
    unsigned long total;
    const char *out_path;
    int attempt;

    LOGI("=== metadump.so constructor called ===");

    /* Verify we're in the target process */
    cmdfd = open("/proc/self/cmdline", O_RDONLY);
    if (cmdfd < 0) { LOGE("cannot open /proc/self/cmdline"); return; }
    ssize_t n = read(cmdfd, cmdline, sizeof(cmdline) - 1);
    close(cmdfd);
    if (n <= 0) { LOGE("empty cmdline"); return; }

    if (!strstr(cmdline, TARGET_PROCESS)) {
        LOGI("not target process, exiting");
        return;
    }
    LOGI("target process confirmed");

    /* Poll for dmabuf:METADATA */
    for (attempt = 0; attempt < POLL_MAX_RETRIES; attempt++) {
        FILE *maps = fopen("/proc/self/maps", "r");
        if (!maps) { LOGE("cannot open /proc/self/maps"); return; }

        char line[512];
        int found = 0;
        unsigned long start = 0, end = 0;
        char path[256] = {0};

        while (fgets(line, sizeof(line), maps)) {
            char perms[8];
            int fields = sscanf(line, "%lx-%lx %4s %*x %*s %*d %255s",
                               &start, &end, perms, path);
            if (fields >= 3 && strstr(path, "dmabuf:METADATA")) {
                found = 1;
                break;
            }
        }
        fclose(maps);

        if (found) {
            LOGI("dmabuf:METADATA found at %lx-%lx (attempt %d)", start, end, attempt);
            goto dump;
        }

        if (attempt == 0) {
            LOGI("dmabuf:METADATA not mapped yet, polling...");
        }
        if (attempt % 10 == 1 && attempt > 0) {
            LOGI("poll attempt %d...", attempt);
        }
        usleep(POLL_INTERVAL_MS);
    }

    LOGE("dmabuf:METADATA never appeared after %d attempts", POLL_MAX_RETRIES);
    return;

dump:
    /* Open output file */
    out_fd = -1;
    out_path = NULL;
    for (int i = 0; OUTPUT_PATHS[i] != NULL; i++) {
        /* Ensure parent dir exists */
        char dir[256];
        strncpy(dir, OUTPUT_PATHS[i], sizeof(dir) - 1);
        dir[sizeof(dir) - 1] = '\0';
        char *slash = strrchr(dir, '/');
        if (slash) { *slash = '\0'; mkdir(dir, 0755); }

        out_fd = open(OUTPUT_PATHS[i], O_WRONLY | O_CREAT | O_TRUNC, 0600);
        if (out_fd >= 0) {
            out_path = OUTPUT_PATHS[i];
            break;
        }
    }
    if (out_fd < 0) { LOGE("all output paths failed"); return; }
    LOGI("writing to %s", out_path);

    /* Dump all dmabuf:METADATA regions */
    total = 0;
    char dump_line[512];
    FILE *maps = fopen("/proc/self/maps", "r");
    if (!maps) { close(out_fd); return; }

    while (fgets(dump_line, sizeof(dump_line), maps)) {
        unsigned long s, e;
        char d_perms[8] = {0}, d_path[256] = {0};
        if (sscanf(dump_line, "%lx-%lx %4s %*x %*s %*d %255s", &s, &e, d_perms, d_path) < 3)
            continue;
        if (!strstr(d_path, "dmabuf:METADATA")) continue;

        size_t size = e - s;
        const char *ptr = (const char *)(unsigned long)s;
        size_t remaining = size;
        while (remaining > 0) {
            size_t chunk = remaining > 4096 ? 4096 : remaining;
            ssize_t written = write(out_fd, ptr, chunk);
            if (written <= 0) break;
            ptr += written;
            remaining -= written;
            total += written;
        }
        LOGI("dumped %lx-%lx (%zu bytes)", s, e, size);
    }
    fclose(maps);
    close(out_fd);

    LOGI("=== DUMP COMPLETE: %lu bytes to %s ===", total, out_path);

    /* Write marker */
    int marker = open(MARKER_PATH, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (marker >= 0) {
        char buf[128];
        int len = snprintf(buf, sizeof(buf),
                          "dumped %lu bytes to %s\n", total, out_path);
        write(marker, buf, len);
        close(marker);
    }
}
