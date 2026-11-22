When you code, make generous use of 
assert! assert_eq! assert_ne! crate::invariant_assert!, etc.

Those will strengthen unit tests. Use it to make preconditions, postconditions, and invariants explicit in your code.

It is also worth considering using the following pattern where it makes sense:

assert!(condition, "...");
if !condition {
    return Err(...); // or some other error-recovery attempt
}

This way you still have a clean fallback in production environment in case if there is a bug that unit tests didn't catch. Of course this has a performance cost, so it should be used judiciously.

So for example, in a bubble sort:

```rust
fn bubble_sort(arr: * mut [i32]) -> Result<(), &'static str> {
    assert!(!arr.is_null(), "array pointer is null");// this is a precondition
    if arr.is_null() {
        return Err("array pointer is null"); // this should never happen but we handle it gracefully anyway
    }
    let n = arr.len();
    for i in 0..n {
        for j in 0..n - 1 - i {
            assert!(j < n - 1, "index out of bounds");
            if arr[j] > arr[j + 1] {
                arr.swap(j, j + 1);
            }
        }
    }
    assert!(is_sorted(arr), "array is not sorted"); // postcondition
    Ok(())
}
```