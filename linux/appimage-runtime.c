#define _GNU_SOURCE
#include <dlfcn.h>
#include <link.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static void *required_symbol(void *library, const char *name)
{
    void *symbol = dlsym(library, name);
    if (!symbol) fprintf(stderr, "Missing runtime function %s: %s\n", name, dlerror());
    return symbol;
}

static int check_runtime(void)
{
    void *webkit = dlopen("libwebkit2gtk-4.1.so.0", RTLD_NOW | RTLD_LOCAL);
    if (!webkit) {
        fprintf(stderr, "Cannot load WebKitGTK 4.1: %s\n", dlerror());
        return 1;
    }
    void *gst = dlopen("libgstreamer-1.0.so.0", RTLD_NOW | RTLD_LOCAL);
    if (!gst) {
        fprintf(stderr, "Cannot load GStreamer: %s\n", dlerror());
        return 1;
    }
    int (*init)(int *, char ***, void **) = required_symbol(gst, "gst_init_check");
    void *(*make)(const char *, const char *) = required_symbol(gst, "gst_element_factory_make");
    void (*unref)(void *) = required_symbol(gst, "g_object_unref");
    if (!init || !make || !unref) return 1;
    if (!init(NULL, NULL, NULL)) {
        fputs("Cannot initialize GStreamer\n", stderr);
        return 1;
    }
    const char *elements[] = {
        "appsrc", "appsink", "playbin", "decodebin", "autoaudiosink",
        "audioconvert", "audioresample", "volume", "scaletempo", "qtdemux", "videoconvert"
    };
    for (size_t i = 0; i < sizeof(elements) / sizeof(elements[0]); i++) {
        void *element = make(elements[i], NULL);
        if (!element) {
            fprintf(stderr, "Missing or incompatible GStreamer element: %s\n", elements[i]);
            return 1;
        }
        unref(element);
    }
    struct link_map *map = NULL;
    if (dlinfo(webkit, RTLD_DI_LINKMAP, &map) != 0 || !map) {
        fprintf(stderr, "Cannot locate WebKitGTK: %s\n", dlerror());
        return 1;
    }
    char *path = realpath(map->l_name, NULL);
    if (!path) {
        perror("Cannot resolve WebKitGTK path");
        return 1;
    }
    char *slash = strrchr(path, '/');
    if (!slash) {
        free(path);
        return 1;
    }
    *slash = '\0';
    puts(path);
    free(path);
    return 0;
}

static int show_error(const char *text)
{
    fputs(text, stderr);
    fputc('\n', stderr);
    void *gtk = dlopen("libgtk-3.so.0", RTLD_NOW | RTLD_LOCAL);
    if (!gtk) {
        fprintf(stderr, "Cannot display dependency instructions: %s\n", dlerror());
        return 1;
    }
    int (*init)(int *, char ***) = required_symbol(gtk, "gtk_init_check");
    void *(*dialog)(void *, int, int, int, const char *, ...) = required_symbol(gtk, "gtk_message_dialog_new");
    void (*title)(void *, const char *) = required_symbol(gtk, "gtk_window_set_title");
    int (*run)(void *) = required_symbol(gtk, "gtk_dialog_run");
    void (*destroy)(void *) = required_symbol(gtk, "gtk_widget_destroy");
    if (!init || !dialog || !title || !run || !destroy) return 1;
    if (!init(NULL, NULL)) {
        fputs("Cannot display dependency instructions: no GTK display\n", stderr);
        return 1;
    }
    // GTK 3 ABI values; this helper must start even when GTK is not installed.
    enum { DIALOG_MODAL = 1, MESSAGE_ERROR = 3, BUTTONS_CLOSE = 2 };
    void *window = dialog(NULL, DIALOG_MODAL, MESSAGE_ERROR, BUTTONS_CLOSE, "%s", text);
    title(window, "OpenResearch needs multimedia libraries");
    run(window);
    destroy(window);
    return 1;
}

int main(int argc, char **argv)
{
    if (argc == 2 && strcmp(argv[1], "check") == 0) return check_runtime();
    if (argc == 3 && strcmp(argv[1], "error") == 0) return show_error(argv[2]);
    fputs("Usage: orx-runtime check | error <message>\n", stderr);
    return 1;
}
