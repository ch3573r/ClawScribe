//! Conservative CPU/prompt allocation defaults for a concurrent meeting app.
pub fn thread_budget(logical_threads: usize) -> i32 {
    logical_threads.saturating_sub(1).clamp(1, 4) as i32
}
pub fn prompt_batch_size(context_size: u32) -> usize { context_size.clamp(1, 512) as usize }
pub fn validate_prompt(context_size: u32, prompt_tokens: usize, max_tokens: i32) -> Result<(), &'static str> {
    if max_tokens <= 0 { return Err("Maximum output tokens must be positive"); }
    if context_size == 0 || prompt_tokens == 0 || prompt_tokens >= context_size as usize {
        return Err("Prompt leaves no room for an answer in the local model context; shorten the input or increase the configured context");
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cpu_budget_reserves_capacity_and_handles_unknown_hardware() {
        assert_eq!((thread_budget(0),thread_budget(1),thread_budget(2),thread_budget(12)), (1,1,1,4));
    }
    #[test]
    fn prompt_batches_do_not_grow_with_large_contexts() {
        assert_eq!(prompt_batch_size(128), 128);
        assert_eq!(prompt_batch_size(65536), 512);
    }
    #[test]
    fn prompt_bounds_are_checked_before_native_decoding() {
        assert!(validate_prompt(2048, 200, 100).is_ok());
        for (context,prompt,maximum) in [(0,1,1),(2048,2048,1),(2048,0,1),(2048,10,0)] {
            assert!(validate_prompt(context,prompt,maximum).is_err());
        }
    }
}
