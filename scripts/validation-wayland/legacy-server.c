/* A protocol fixture, not a compositor. Uses the host libwayland server to
 * exercise version negotiation and actual shared-memory pixel copying. */
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <wayland-server.h>
#include "xdg-output-server.h"
#include "screencopy-server.h"

static int output_version, xdg_version, copy_version, no_outputs;
static const char *failure;
static struct wl_interface output_interface;
static struct wl_message output_events[6];

static void destroy_resource(struct wl_client *client, struct wl_resource *resource) {
    (void)client;
    wl_resource_destroy(resource);
}

static const struct wl_output_interface output_requests = { destroy_resource };

static void bind_output(struct wl_client *client, void *data, uint32_t version, uint32_t id) {
    (void)data;
    struct wl_resource *output = wl_resource_create(client, &output_interface, version, id);
    wl_resource_set_implementation(output, &output_requests, NULL, NULL);
    wl_output_send_geometry(output, 7, 11, 300, 200, WL_OUTPUT_SUBPIXEL_UNKNOWN,
                            "Fixture", "Legacy", WL_OUTPUT_TRANSFORM_NORMAL);
    wl_output_send_mode(output, WL_OUTPUT_MODE_CURRENT | WL_OUTPUT_MODE_PREFERRED, 32, 16, 60000);
    if (version >= 2) {
        wl_output_send_scale(output, 2);
        wl_output_send_done(output);
    }
    if (version >= 4) {
        wl_resource_post_event(output, 4, "LEGACY-1");
        wl_resource_post_event(output, 5, "Legacy Wayland test output");
        wl_output_send_done(output);
    }
}

static const struct zxdg_output_v1_interface xdg_output_requests = { destroy_resource };

static void get_xdg_output(struct wl_client *client, struct wl_resource *manager,
                           uint32_t id, struct wl_resource *output) {
    uint32_t version = wl_resource_get_version(manager);
    struct wl_resource *xdg = wl_resource_create(client, &zxdg_output_v1_interface, version, id);
    wl_resource_set_implementation(xdg, &xdg_output_requests, NULL, NULL);
    zxdg_output_v1_send_logical_position(xdg, 7, 11);
    zxdg_output_v1_send_logical_size(xdg, 16, 8);
    if (version >= 2) {
        zxdg_output_v1_send_name(xdg, "LEGACY-1");
        zxdg_output_v1_send_description(xdg, "Legacy Wayland test output");
    }
    if (version < 3) {
        zxdg_output_v1_send_done(xdg);
    } else {
        wl_output_send_done(output);
    }
}

static const struct zxdg_output_manager_v1_interface xdg_requests = {
    destroy_resource, get_xdg_output
};

static void bind_xdg(struct wl_client *client, void *data, uint32_t version, uint32_t id) {
    (void)data;
    struct wl_resource *manager = wl_resource_create(client, &zxdg_output_manager_v1_interface, version, id);
    wl_resource_set_implementation(manager, &xdg_requests, NULL, NULL);
}

struct frame_size { int width, height; };

static void destroy_frame_data(struct wl_resource *frame) {
    free(wl_resource_get_user_data(frame));
}

static void copy_frame(struct wl_client *client, struct wl_resource *frame,
                       struct wl_resource *buffer_resource) {
    (void)client;
    if (!strcmp(failure, "copy")) {
        zwlr_screencopy_frame_v1_send_failed(frame);
        return;
    }
    struct frame_size *size = wl_resource_get_user_data(frame);
    struct wl_shm_buffer *buffer = wl_shm_buffer_get(buffer_resource);
    if (!buffer || wl_shm_buffer_get_width(buffer) != size->width ||
        wl_shm_buffer_get_height(buffer) != size->height ||
        wl_shm_buffer_get_format(buffer) != WL_SHM_FORMAT_XRGB8888) {
        wl_resource_post_error(frame, ZWLR_SCREENCOPY_FRAME_V1_ERROR_INVALID_BUFFER,
                               "wrong fixture buffer dimensions or format");
        return;
    }
    wl_shm_buffer_begin_access(buffer);
    unsigned char *pixels = wl_shm_buffer_get_data(buffer);
    int stride = wl_shm_buffer_get_stride(buffer);
    for (int y = 0; y < size->height; y++) {
        uint32_t *row = (uint32_t *)(pixels + y * stride);
        for (int x = 0; x < size->width; x++) {
            int image_y = !strcmp(failure, "invert") ? size->height - 1 - y : y;
            row[x] = 0xff000000u | ((19u + x) << 16) | ((41u + image_y) << 8) | 73u;
        }
    }
    wl_shm_buffer_end_access(buffer);
    zwlr_screencopy_frame_v1_send_flags(frame, !strcmp(failure, "invert") ?
                                      ZWLR_SCREENCOPY_FRAME_V1_FLAGS_Y_INVERT : 0);
    zwlr_screencopy_frame_v1_send_ready(frame, 0, 1, 0);
}

static const struct zwlr_screencopy_frame_v1_interface frame_requests = {
    copy_frame, destroy_resource, copy_frame
};

static void new_frame(struct wl_client *client, struct wl_resource *manager,
                      uint32_t id, int width, int height) {
    struct wl_resource *frame = wl_resource_create(client, &zwlr_screencopy_frame_v1_interface,
                                                   wl_resource_get_version(manager), id);
    struct frame_size *size = malloc(sizeof(*size));
    assert(size);
    size->width = width;
    size->height = height;
    wl_resource_set_implementation(frame, &frame_requests, size, destroy_frame_data);
    if (!strcmp(failure, "early")) {
        zwlr_screencopy_frame_v1_send_failed(frame);
        return;
    }
    uint32_t format = !strcmp(failure, "unknown") ? UINT32_MAX : WL_SHM_FORMAT_XRGB8888;
    zwlr_screencopy_frame_v1_send_buffer(frame, format,
                                        width, height, width * 4);
    if (wl_resource_get_version(frame) >= 3) {
        zwlr_screencopy_frame_v1_send_buffer_done(frame);
    }
}

static void capture_output(struct wl_client *client, struct wl_resource *manager,
                           uint32_t id, int32_t cursor, struct wl_resource *output) {
    (void)cursor; (void)output;
    new_frame(client, manager, id, 32, 16);
}

static void capture_region(struct wl_client *client, struct wl_resource *manager,
                           uint32_t id, int32_t cursor, struct wl_resource *output,
                           int32_t x, int32_t y, int32_t width, int32_t height) {
    (void)cursor; (void)output;
    if (x < 0 || y < 0 || x + width > 16 || y + height > 8 || width < 1 || height < 1) {
        wl_resource_post_error(manager, 0, "region outside fixture output");
        return;
    }
    new_frame(client, manager, id, width * 2, height * 2);
}

static const struct zwlr_screencopy_manager_v1_interface copy_requests = {
    capture_output, capture_region, destroy_resource
};

static void bind_copy(struct wl_client *client, void *data, uint32_t version, uint32_t id) {
    (void)data;
    struct wl_resource *manager = wl_resource_create(client, &zwlr_screencopy_manager_v1_interface, version, id);
    wl_resource_set_implementation(manager, &copy_requests, NULL, NULL);
}

int main(int argc, char **argv) {
    if (argc != 7) {
        fprintf(stderr, "usage: legacy-server SOCKET OUTPUT_VERSION XDG_VERSION COPY_VERSION NO_OUTPUTS FAILURE\n");
        return 2;
    }
    output_version = atoi(argv[2]);
    xdg_version = atoi(argv[3]);
    copy_version = atoi(argv[4]);
    no_outputs = atoi(argv[5]);
    failure = argv[6];
    assert(output_version >= 1 && output_version <= 4);
    assert(xdg_version >= 0 && xdg_version <= 3);
    assert(copy_version >= 0 && copy_version <= 3);
    /* libwayland 1.18's exported wl_output descriptor ends at v3. Extend a
     * private copy, allowing v4 clients without upgrading the offline image. */
    output_interface = wl_output_interface;
    output_interface.version = 4;
    for (int i = 0; i < 4; i++) output_events[i] = wl_output_interface.events[i];
    output_events[4] = (struct wl_message){ "name", "4s", NULL };
    output_events[5] = (struct wl_message){ "description", "4s", NULL };
    output_interface.event_count = 6;
    output_interface.events = output_events;
    struct wl_display *display = wl_display_create();
    assert(display);
    assert(wl_display_init_shm(display) == 0);
    if (!no_outputs) assert(wl_global_create(display, &output_interface, output_version, NULL, bind_output));
    if (xdg_version) assert(wl_global_create(display, &zxdg_output_manager_v1_interface, xdg_version, NULL, bind_xdg));
    if (copy_version) assert(wl_global_create(display, &zwlr_screencopy_manager_v1_interface, copy_version, NULL, bind_copy));
    assert(wl_display_add_socket(display, argv[1]) == 0);
    puts("READY");
    fflush(stdout);
    wl_display_run(display);
    wl_display_destroy_clients(display);
    wl_display_destroy(display);
    return 0;
}
