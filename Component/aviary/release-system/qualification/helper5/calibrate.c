// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>
static pthread_barrier_t barrier;
static pthread_mutex_t print_lock=PTHREAD_MUTEX_INITIALIZER;
static FILE *expect;
static const char *root;
static void *worker(void *arg) {
    long n=(long)arg; char dir[4096],file[4096];
    if(snprintf(dir,sizeof dir,"%s/thread-%ld",root,n)>=(int)sizeof dir)abort();
    if(mkdir(dir,0700))abort();
    if(snprintf(file,sizeof file,"%s/data",dir)>=(int)sizeof file)abort();
    int d=open(dir,O_RDONLY|O_DIRECTORY), f=open(file,O_RDWR|O_CREAT|O_EXCL,0600);
    if(d<0||f<0||write(f,"x",1)!=1)abort();
    pthread_mutex_lock(&print_lock);
    fprintf(expect,"{\"tid\":%ld,\"dirfd\":%d,\"filefd\":%d,\"dir\":\"%s\",\"file\":\"%s\"}\n",syscall(SYS_gettid),d,f,dir,file);
    fflush(expect);pthread_mutex_unlock(&print_lock);
    pthread_barrier_wait(&barrier);
    for(int i=0;i<3;i++) {
        if(syscall(SYS_fsync,(long)d)!=0)abort();
        if(syscall(SYS_fsync,(long)f)!=0)abort();
        errno=0;if(syscall(SYS_fsync,-1L)!=-1||errno!=EBADF)abort();
    }
    close(f);close(d);return NULL;
}
int main(int argc,char **argv) {
    if(argc!=2)return 2;
    root=argv[1];
    char name[4096];
    if(snprintf(name,sizeof name,"%s/expected.jsonl",root)>=(int)sizeof name)return 2;
    expect=fopen(name,"wx");if(!expect)return 2;
    pthread_barrier_init(&barrier,NULL,2);pthread_t a,b;
    if(pthread_create(&a,NULL,worker,(void*)0)||pthread_create(&b,NULL,worker,(void*)1))return 2;
    pthread_join(a,NULL);pthread_join(b,NULL);fclose(expect);return 0;
}
