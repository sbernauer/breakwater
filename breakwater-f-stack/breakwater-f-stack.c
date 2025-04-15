#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <strings.h>
#include <sys/types.h>
#include <sys/socket.h>
#include <arpa/inet.h>
#include <errno.h>
#include <assert.h>
#include <sys/ioctl.h>
#include <sys/resource.h>

#include "ff_config.h"
#include "ff_api.h"

#include "breakwater-f-stack.h"
#include "framebuffer.h"
#include "parser.h"

#define MAX_EVENTS 512

/* kevent set */
struct kevent kevSet;
/* events */
struct kevent events[MAX_EVENTS];
/* kq */
int kq;
int sockfd;
#ifdef INET6
int sockfd6;
#endif

// The main read buffer
char buf[32 * 1024];

// Array of pointers to client structures
client_state **clients;
// Max number of FDs (from ulimit -n)
size_t max_clients;


// Get the system limit for file descriptors
size_t get_max_fds() {
    struct rlimit limit;
    if (getrlimit(RLIMIT_NOFILE, &limit) != 0) {
        perror("getrlimit failed");
        exit(EXIT_FAILURE);
    }
    return limit.rlim_cur;
}

// Initialize client array
void init_clients() {
    max_clients = get_max_fds();
    clients = calloc(max_clients, sizeof(client_state *));
    if (!clients) {
        perror("Memory allocation for clients failed");
        exit(EXIT_FAILURE);
    }
    printf("Allocated space for %zu client connections (~%zu KB)\n",
           max_clients, (max_clients * sizeof(client_state *)) / 1024);
}

// Add a client state
void add_client(int fd) {
    if (fd < 0 || fd >= max_clients) {
        fprintf(stderr, "Invalid fd: %d\n", fd);
        return;
    }

    if (clients[fd] == NULL) {
        clients[fd] = malloc(sizeof(client_state));
        if (!clients[fd]) {
            perror("Failed to allocate client state");
            return;
        }
        memset(clients[fd], 0, sizeof(client_state));
    }
}

// Lookup client state by fd
client_state *get_client(int fd) {
    // if (fd < 0 || fd >= max_clients) {
    //     return NULL;
    // }
    return clients[fd];
}

// Remove a client state
void remove_client(int fd) {
    if (fd < 0 || fd >= max_clients || clients[fd] == NULL) {
        return;
    }
    free(clients[fd]);
    clients[fd] = NULL;
}

// Cleanup memory
void cleanup_clients() {
    for (size_t i = 0; i < max_clients; i++) {
        if (clients[i]) {
            free(clients[i]);
        }
    }
    free(clients);
}


int loop(void *arg)
{
    struct framebuffer *framebuffer = (struct framebuffer *)arg;

    /* Wait for events to happen */
    int nevents = ff_kevent(kq, NULL, 0, events, MAX_EVENTS, NULL);
    int i;

    if (nevents < 0) {
        printf("ff_kevent failed:%d, %s\n", errno, strerror(errno));
        return -1;
    }

    for (i = 0; i < nevents; ++i) {
        struct kevent event = events[i];
        int clientfd = (int)event.ident;

        /* Handle disconnect */
        if (event.flags & EV_EOF) {
            /* Simply close socket */
            ff_close(clientfd);
#ifdef INET6
        } else if (clientfd == sockfd || clientfd == sockfd6) {
#else
        } else if (clientfd == sockfd) {
#endif
            int available = (int)event.data;
            do {
                int nclientfd = ff_accept(clientfd, NULL, NULL);
                if (nclientfd < 0) {
                    printf("ff_accept failed:%d, %s\n", errno, strerror(errno));
                    break;
                }

                // printf("Got new client connection");

                // Add to clients array
                add_client(nclientfd);
                // Add to event list
                EV_SET(&kevSet, nclientfd, EVFILT_READ, EV_ADD, 0, 0, NULL);

                if(ff_kevent(kq, &kevSet, 1, NULL, 0, NULL) < 0) {
                    printf("ff_kevent error:%d, %s\n", errno, strerror(errno));
                    return -1;
                }

                available--;
            } while (available);
        } else if (event.filter == EVFILT_READ) {
            client_state *client = get_client(clientfd);
            ssize_t readlen = ff_read(clientfd, buf, sizeof(buf));
            client->bytes_parsed += readlen;

            size_t bytes_parsed = parse(buf, readlen, framebuffer, clientfd);
        } else {
            printf("unknown event: %8.8X\n", event.flags);
        }
    }

    return 0;
}

int main(int argc, char * argv[])
{
    int err = 0;

    struct framebuffer* framebuffer;
    if((err = create_fb(&framebuffer, WIDTH, HEIGHT, SHARED_MEMORY_NAME))) {
		fprintf(stderr, "Failed to allocate framebuffer: %s\n", strerror(err));
        return err;
	}

    // for (uint16_t x = 0; x <= 150; x++) {
    //     for (uint16_t y = 0; y <= 50; y++) {
    //         fb_set(framebuffer, x, y, 0x00ff0000);
    //     }
    // }

    ff_init(argc, argv);

    kq = ff_kqueue();
    if (kq < 0) {
        printf("ff_kqueue failed, errno:%d, %s\n", errno, strerror(errno));
        exit(1);
    }

    sockfd = ff_socket(AF_INET, SOCK_STREAM, 0);
    if (sockfd < 0) {
        printf("ff_socket failed, sockfd:%d, errno:%d, %s\n", sockfd, errno, strerror(errno));
        exit(1);
    }

    /* Set non blocking */
    int on = 1;
    ff_ioctl(sockfd, FIONBIO, &on);

    struct sockaddr_in my_addr;
    bzero(&my_addr, sizeof(my_addr));
    my_addr.sin_family = AF_INET;
    my_addr.sin_port = htons(SERVER_PORT);
    my_addr.sin_addr.s_addr = htonl(INADDR_ANY);

    int ret = ff_bind(sockfd, (struct linux_sockaddr *)&my_addr, sizeof(my_addr));
    if (ret < 0) {
        printf("ff_bind failed, sockfd:%d, errno:%d, %s\n", sockfd, errno, strerror(errno));
        exit(1);
    }

    ret = ff_listen(sockfd, MAX_EVENTS);
    if (ret < 0) {
        printf("ff_listen failed, sockfd:%d, errno:%d, %s\n", sockfd, errno, strerror(errno));
        exit(1);
    }

    EV_SET(&kevSet, sockfd, EVFILT_READ, EV_ADD, 0, MAX_EVENTS, NULL);
    /* Update kqueue */
    ff_kevent(kq, &kevSet, 1, NULL, 0, NULL);

#ifdef INET6
    sockfd6 = ff_socket(AF_INET6, SOCK_STREAM, 0);
    if (sockfd6 < 0) {
        printf("ff_socket failed, sockfd6:%d, errno:%d, %s\n", sockfd6, errno, strerror(errno));
        exit(1);
    }

    struct sockaddr_in6 my_addr6;
    bzero(&my_addr6, sizeof(my_addr6));
    my_addr6.sin6_family = AF_INET6;
    my_addr6.sin6_port = htons(SERVER_PORT);
    my_addr6.sin6_addr = in6addr_any;

    ret = ff_bind(sockfd6, (struct linux_sockaddr *)&my_addr6, sizeof(my_addr6));
    if (ret < 0) {
        printf("ff_bind failed, sockfd6:%d, errno:%d, %s\n", sockfd6, errno, strerror(errno));
        exit(1);
    }

    ret = ff_listen(sockfd6, MAX_EVENTS);
    if (ret < 0) {
        printf("ff_listen failed, sockfd6:%d, errno:%d, %s\n", sockfd6, errno, strerror(errno));
        exit(1);
    }

    EV_SET(&kevSet, sockfd6, EVFILT_READ, EV_ADD, 0, MAX_EVENTS, NULL);
    ret = ff_kevent(kq, &kevSet, 1, NULL, 0, NULL);
    if (ret < 0) {
        printf("ff_kevent failed:%d, %s\n", errno, strerror(errno));
        exit(1);
    }
#endif

    init_clients();
    ff_run(loop, framebuffer);

    cleanup_clients();
    return 0;
}
