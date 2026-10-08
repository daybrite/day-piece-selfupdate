// Copyright © The Daybrite Project
// SPDX-License-Identifier: MPL-2.0
#import "Protocol.h"
#include <stdlib.h>
#include <string.h>

char *dysu_config(void) {
    @autoreleasepool {
        NSBundle *bundle = NSBundle.mainBundle;
        NSString *inbox = [[NSTemporaryDirectory() stringByAppendingPathComponent:@"day-selfupdate-inbox"]
            stringByAppendingPathComponent:bundle.bundleIdentifier ?: @"unbundled"];
        [NSFileManager.defaultManager createDirectoryAtPath:inbox withIntermediateDirectories:YES attributes:nil error:nil];
        NSDictionary *config = @{
            @"application_id": bundle.bundleIdentifier ?: @"",
            @"build": [bundle objectForInfoDictionaryKey:@"CFBundleVersion"] ?: @"0",
            @"version": [bundle objectForInfoDictionaryKey:@"CFBundleShortVersionString"] ?: @"",
            @"key": [bundle objectForInfoDictionaryKey:@"DYSUPublicKey"] ?: @"",
            @"inbox": inbox,
            @"bundle": bundle.bundlePath,
            @"repository": [bundle objectForInfoDictionaryKey:@"DYSURepository"] ?: @"",
            @"target": [bundle objectForInfoDictionaryKey:@"DYSUTarget"] ?: @"",
            @"development": @([[bundle objectForInfoDictionaryKey:@"DYSUDevelopment"] boolValue])
        };
        NSData *data = [NSJSONSerialization dataWithJSONObject:config options:0 error:nil];
        return strdup([[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding].UTF8String);
    }
}

// Blocking worker-thread API. XPC callbacks run on Foundation's queues, never on Day's UI.
char *dysu_begin(const char *inbox) {
    @autoreleasepool {
        NSString *service = [NSBundle.mainBundle objectForInfoDictionaryKey:@"DYSUServiceIdentifier"];
        NSString *requirement = [NSBundle.mainBundle objectForInfoDictionaryKey:@"DYSUServiceRequirement"];
        if (!service || !requirement) return strdup("not-packaged");
        NSXPCConnection *connection = [[NSXPCConnection alloc] initWithServiceName:service];
        if (@available(macOS 13.0, *)) [connection setCodeSigningRequirement:requirement];
        else return strdup("unsupported-system");
        connection.remoteObjectInterface = [NSXPCInterface interfaceWithProtocol:@protocol(DYSUInstaller)];
        dispatch_semaphore_t done = dispatch_semaphore_create(0);
        NSLock *lock = [NSLock new];
        __block NSString *result = nil;
        void (^finish)(NSString *) = ^(NSString *value) {
            [lock lock];
            if (!result) { result = [value copy]; dispatch_semaphore_signal(done); }
            [lock unlock];
        };
        connection.invalidationHandler = ^{ finish(@"connection-invalidated"); };
        connection.interruptionHandler = ^{ finish(@"connection-interrupted"); };
        [connection resume];
        id<DYSUInstaller> remote = [connection remoteObjectProxyWithErrorHandler:^(NSError *error) {
            NSLog(@"selfupdate IPC: %@", error);
            finish(@"connection-rejected");
        }];
        [remote prepareInbox:[NSString stringWithUTF8String:inbox] reply:finish];
        if (dispatch_semaphore_wait(done, dispatch_time(DISPATCH_TIME_NOW, 45 * NSEC_PER_SEC))) finish(@"timeout");
        [lock lock]; NSString *answer = result; [lock unlock];
        [connection invalidate];
        return strdup(answer.UTF8String);
    }
}

void dysu_free(char *value) { free(value); }
