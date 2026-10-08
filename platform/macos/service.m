// Copyright © The Daybrite Project
// SPDX-License-Identifier: MPL-2.0
#import "Protocol.h"

@interface Installer : NSObject <DYSUInstaller>
@property(nonatomic) pid_t clientPID;
@end

@implementation Installer
- (void)prepareInbox:(NSString *)inbox reply:(void (^)(NSString *))reply {
    // A connection-scoped PID from Foundation, never a PID supplied by the caller.
    pid_t pid = self.clientPID;
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_UTILITY, 0), ^{
        NSString *host = NSBundle.mainBundle.bundlePath;
        for (int i = 0; i < 3; i++) host = host.stringByDeletingLastPathComponent;
        NSBundle *bundle = [NSBundle bundleWithPath:host];
        NSString *session = [host.stringByDeletingLastPathComponent stringByAppendingPathComponent:
            [@".day-selfupdate-" stringByAppendingString:bundle.bundleIdentifier]];
        NSTask *task = [NSTask new];
        task.executableURL = [NSURL fileURLWithPath:[host stringByAppendingPathComponent:@"Contents/Helpers/day-selfupdate-tool"]];
        task.arguments = @[@"install", inbox, [NSString stringWithFormat:@"%d", pid]];
        NSError *error = nil;
        if (![task launchAndReturnError:&error]) { NSLog(@"installer launch: %@", error); reply(@"launch-failed"); return; }
        // The helper owns all verification and filesystem writes. It holds an exclusive lock.
        // Existing ready files do not count until this child writes its own PID marker.
        NSString *ready = [session stringByAppendingPathComponent:@"ready"];
        NSString *expected = [NSString stringWithFormat:@"%d", task.processIdentifier];
        NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:35];
        while (task.running && [deadline timeIntervalSinceNow] > 0) {
            NSString *value = [NSString stringWithContentsOfFile:ready encoding:NSUTF8StringEncoding error:nil];
            if ([value isEqualToString:expected]) { reply(@"ready"); return; }
            [NSThread sleepForTimeInterval:0.05];
        }
        reply(task.running ? @"prepare-timeout" : @"verification-failed");
    });
}
@end

@interface Delegate : NSObject <NSXPCListenerDelegate>
@end
@implementation Delegate
- (BOOL)listener:(NSXPCListener *)listener shouldAcceptNewConnection:(NSXPCConnection *)connection {
    (void)listener;
    NSString *requirement = [NSBundle.mainBundle objectForInfoDictionaryKey:@"DYSUClientRequirement"];
    if (!requirement || connection.processIdentifier <= 1) return NO;
    if (@available(macOS 13.0, *)) [connection setCodeSigningRequirement:requirement];
    else return NO;
    Installer *installer = [Installer new];
    installer.clientPID = connection.processIdentifier;
    connection.exportedInterface = [NSXPCInterface interfaceWithProtocol:@protocol(DYSUInstaller)];
    connection.exportedObject = installer;
    [connection resume];
    return YES;
}
@end

int main(void) {
    @autoreleasepool {
        NSXPCListener *listener = NSXPCListener.serviceListener;
        Delegate *delegate = [Delegate new];
        listener.delegate = delegate;
        [listener resume];
    }
    return 0;
}
