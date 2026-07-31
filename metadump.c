/**
 * metadump.c - In-process dmabuf metadata dumper for DarakuGear
 * 
 * Zygisk module: constructor auto-runs on dlopen in target process.
 * Scans /proc/self/maps for dmabuf:METADATA regions and dumps them.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>

#define TARGET_PROCESS "com.pinkcore.darakugear"

__attribute__((constructor))
void dump_metadata(void) {
    char cmdline[256] = {0};
    FILE *maps;
    char line[512];
    int out_fd;
    unsigned long total = 0;
    
    /* Check if we're in the target process */
    int cmdfd = open("/proc/self/cmdline", O_RDONLY);
    if (cmdfd < 0) return;
    read(cmdfd, cmdline, sizeof(cmdline) - 1);
    close(cmdfd);
    
    if (!strstr(cmdline, TARGET_PROCESS)) return;
    
    /* Open output file */
    out_fd = open("/data/local/tmp/metadata_dump.bin",
                  O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (out_fd < 0) return;
    
    /* Open process memory map */
    maps = fopen("/proc/self/maps", "r");
    if (!maps) { close(out_fd); return; }
    
    while (fgets(line, sizeof(line), maps)) {
        unsigned long start, end;
        char perms[8] = {0};
        char path[256] = {0};
        int n;
        
        n = sscanf(line, "%lx-%lx %4s %*x %*s %*d %255s",
                   &start, &end, perms, path);
        if (n < 2) continue;
        
        /* Match dmabuf:METADATA regions */
        if (n >= 3 && strstr(path, "dmabuf:METADATA")) {
            size_t size = end - start;
            ssize_t written = write(out_fd, (const void *)start, size);
            if (written > 0) total += written;
        }
    }
    
    fclose(maps);
    close(out_fd);
    
    /* Confirm dump */
    int marker = open("/data/local/tmp/metadata_dump.done",
                      O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (marker >= 0) {
        char buf[64];
        snprintf(buf, sizeof(buf), "dumped %lu bytes\n", total);
        write(marker, buf, strlen(buf));
        close(marker);
    }
}
