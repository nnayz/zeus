//! `UNUserNotificationCenter` bridge for actionable agent notifications.
//!
//! The center is deliberately inert for a raw
//! `cargo run`; UserNotifications aborts when no application bundle exists.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, ProtocolObject};
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_foundation::{
    NSArray, NSBundle, NSDictionary, NSError, NSObject, NSObjectProtocol, NSSet, NSString,
};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification, UNNotificationAction,
    UNNotificationActionOptions, UNNotificationCategory, UNNotificationCategoryOptions,
    UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
    UNNotificationSound, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};
use tokio::sync::mpsc;

use crate::notifications::{
    APPROVE_ACTION_ID, ActionData, DENY_ACTION_ID, NotificationRequest as DiriNotification,
    PERMISSION_CATEGORY_ID, SendTextCommand, command_for_action,
};

const ACTION_DATA_KEY: &str = "diriActionData";

pub struct NativeNotifier {
    inner: Option<NativeNotifierInner>,
}

struct NativeNotifierInner {
    center: Retained<UNUserNotificationCenter>,
    // `delegate` is weak on UNUserNotificationCenter.
    _delegate: Retained<NotificationDelegate>,
    actions: Arc<Mutex<HashMap<String, ActionData>>>,
    authorization_requested: Cell<bool>,
}

impl NativeNotifier {
    #[must_use]
    pub fn new(action_sender: mpsc::UnboundedSender<SendTextCommand>) -> Self {
        let inner = NativeNotifierInner::new(action_sender);
        Self { inner }
    }

    pub fn post(&self, notification: &DiriNotification) {
        if let Some(inner) = &self.inner {
            inner.post(notification);
        }
    }
}

impl NativeNotifierInner {
    fn new(action_sender: mpsc::UnboundedSender<SendTextCommand>) -> Option<Self> {
        // UserNotifications throws an Objective-C exception outside an app bundle.
        NSBundle::mainBundle().bundleIdentifier()?;

        let center = UNUserNotificationCenter::currentNotificationCenter();
        let actions = Arc::new(Mutex::new(HashMap::new()));
        let delegate = NotificationDelegate::new(action_sender, Arc::clone(&actions));
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        register_permission_category(&center);

        Some(Self {
            center,
            _delegate: delegate,
            actions,
            authorization_requested: Cell::new(false),
        })
    }

    fn request_authorization(&self) {
        if self.authorization_requested.replace(true) {
            return;
        }
        let completion = RcBlock::new(|_granted: Bool, _error: *mut NSError| {});
        self.center
            .requestAuthorizationWithOptions_completionHandler(
                UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
                &completion,
            );
    }

    fn post(&self, notification: &DiriNotification) {
        self.request_authorization();
        if let Some(action_data) = &notification.action_data {
            self.actions
                .lock()
                .expect("notification action map poisoned")
                .insert(notification.identifier.clone(), action_data.clone());
        }

        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&notification.title));
        content.setBody(&NSString::from_str(&notification.body));
        if let Some(thread) = &notification.thread_identifier {
            content.setThreadIdentifier(&NSString::from_str(thread));
        }
        if notification.action_data.is_some() {
            content.setCategoryIdentifier(&NSString::from_str(PERMISSION_CATEGORY_ID));
        }
        if let Some(action_data) = &notification.action_data
            && let Ok(json) = serde_json::to_string(action_data)
        {
            let key = NSString::from_str(ACTION_DATA_KEY);
            let value = NSString::from_str(&json);
            let typed = NSDictionary::from_slices(&[&*key], &[&*value]);
            // SAFETY: Objective-C lightweight generics are erased. Both key and
            // value are property-list-compatible NSString instances.
            let erased: &NSDictionary =
                unsafe { &*(Retained::as_ptr(&typed).cast::<NSDictionary>()) };
            // SAFETY: the dictionary contains valid NSString keys and values.
            unsafe { content.setUserInfo(erased) };
        }
        if notification.use_system_sound {
            content.setSound(Some(&UNNotificationSound::defaultSound()));
        }

        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(&notification.identifier),
            &content,
            None,
        );
        self.center
            .addNotificationRequest_withCompletionHandler(&request, None);
    }
}

fn register_permission_category(center: &UNUserNotificationCenter) {
    let approve = UNNotificationAction::actionWithIdentifier_title_options(
        &NSString::from_str(APPROVE_ACTION_ID),
        &NSString::from_str("Approve"),
        UNNotificationActionOptions::empty(),
    );
    let deny = UNNotificationAction::actionWithIdentifier_title_options(
        &NSString::from_str(DENY_ACTION_ID),
        &NSString::from_str("Deny"),
        UNNotificationActionOptions::Destructive,
    );
    let category = UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
        &NSString::from_str(PERMISSION_CATEGORY_ID),
        &NSArray::from_retained_slice(&[approve, deny]),
        &NSArray::new(),
        UNNotificationCategoryOptions::empty(),
    );
    center.setNotificationCategories(&NSSet::from_retained_slice(&[category]));
}

struct DelegateIvars {
    action_sender: mpsc::UnboundedSender<SendTextCommand>,
    actions: Arc<Mutex<HashMap<String, ActionData>>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and this class does not
    // implement Drop. UserNotifications may invoke its delegate off-main.
    #[unsafe(super(NSObject))]
    #[ivars = DelegateIvars]
    struct NotificationDelegate;

    unsafe impl NSObjectProtocol for NotificationDelegate {}

    unsafe impl UNUserNotificationCenterDelegate for NotificationDelegate {
        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive_notification_response(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion_handler: &block2::DynBlock<dyn Fn()>,
        ) {
            let identifier = response.notification().request().identifier().to_string();
            let action_id = response.actionIdentifier().to_string();
            let data = self
                .ivars()
                .actions
                .lock()
                .expect("notification action map poisoned")
                .remove(&identifier)
                .or_else(|| action_data_from_response(response));
            if let Some(command) = data
                .as_ref()
                .and_then(|data| command_for_action(&action_id, data))
            {
                let _ = self.ivars().action_sender.send(command);
            }
            completion_handler.call(());
        }

        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present_notification(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            completion_handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            completion_handler.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::List
                | UNNotificationPresentationOptions::Sound,));
        }
    }
);

fn action_data_from_response(response: &UNNotificationResponse) -> Option<ActionData> {
    let user_info = response.notification().request().content().userInfo();
    let key = NSString::from_str(ACTION_DATA_KEY);
    let value = user_info.objectForKey(&*key as &AnyObject)?;
    let json = value.downcast::<NSString>().ok()?.to_string();
    serde_json::from_str(&json).ok()
}

impl NotificationDelegate {
    fn new(
        action_sender: mpsc::UnboundedSender<SendTextCommand>,
        actions: Arc<Mutex<HashMap<String, ActionData>>>,
    ) -> Retained<Self> {
        let this = Self::alloc().set_ivars(DelegateIvars {
            action_sender,
            actions,
        });
        // SAFETY: NSObject's init is its designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}
