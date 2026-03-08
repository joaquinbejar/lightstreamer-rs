/******************************************************************************
   Author: Joaquín Béjar García
   Email: jb@taunais.com
   Date: 16/5/25
******************************************************************************/
use crate::subscription::Subscription;

/// Retrieve a reference to a subscription with the given `id`
pub(crate) fn get_subscription_by_id(
    subscriptions: &[Subscription],
    subscription_id: usize,
) -> Option<&Subscription> {
    subscriptions.iter().find(|sub| sub.id == subscription_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subscription::SubscriptionMode;

    #[test]
    fn test_get_subscription_by_id_found() {
        // Create a test subscription with ID 1
        let mut subscription1 = Subscription::new(
            SubscriptionMode::Merge,
            Some(vec!["item1".to_string()]),
            Some(vec!["field1".to_string()]),
        )
        .expect("Failed to create test subscription1");
        subscription1.id = 1;

        // Create another test subscription with ID 2
        let mut subscription2 = Subscription::new(
            SubscriptionMode::Distinct,
            Some(vec!["item2".to_string()]),
            Some(vec!["field2".to_string()]),
        )
        .expect("Failed to create test subscription2");
        subscription2.id = 2;

        // Create a vector of subscriptions
        let subscriptions = vec![subscription1, subscription2];

        // Test finding subscription with ID 1
        let result = get_subscription_by_id(&subscriptions, 1);
        assert!(result.is_some(), "Subscription with ID 1 should be found");
        assert_eq!(result.map(|s| s.id), Some(1));

        // Test finding subscription with ID 2
        let result = get_subscription_by_id(&subscriptions, 2);
        assert!(result.is_some(), "Subscription with ID 2 should be found");
        assert_eq!(result.map(|s| s.id), Some(2));
    }

    #[test]
    fn test_get_subscription_by_id_not_found() {
        // Create a test subscription with ID 1
        let mut subscription = Subscription::new(
            SubscriptionMode::Merge,
            Some(vec!["item1".to_string()]),
            Some(vec!["field1".to_string()]),
        )
        .expect("Failed to create test subscription");
        subscription.id = 1;

        // Create a vector with one subscription
        let subscriptions = vec![subscription];

        // Test finding a non-existent subscription ID
        let result = get_subscription_by_id(&subscriptions, 999);
        assert!(result.is_none());
    }

    #[test]
    fn test_get_subscription_by_id_empty_list() {
        // Create an empty vector of subscriptions
        let subscriptions: Vec<Subscription> = vec![];

        // Test finding any subscription ID in an empty list
        let result = get_subscription_by_id(&subscriptions, 1);
        assert!(result.is_none());
    }
}
