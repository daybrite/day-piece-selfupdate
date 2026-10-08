// Copyright © The Daybrite Project
// SPDX-License-Identifier: MPL-2.0
#import <Foundation/Foundation.h>

@protocol DYSUInstaller
- (void)prepareInbox:(NSString *)inbox reply:(void (^)(NSString *))reply;
@end
