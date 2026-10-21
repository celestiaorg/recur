//! Go's `sort.SliceStable` (insertion-sorted blocks of 20, then SymMerge).
//! Thread ordering uses a comparator that is not a total order (rows without
//! a timestamp compare equal to everything), so the result depends on the
//! algorithm; this keeps it identical to Go.

pub(crate) fn slice_stable<T>(v: &mut [T], less: impl Fn(&T, &T) -> bool) {
    let n = v.len();
    let mut block = 20;
    let (mut a, mut b) = (0, block);
    while b <= n {
        insertion_sort(v, a, b, &less);
        a = b;
        b += block;
    }
    insertion_sort(v, a, n, &less);
    while block < n {
        a = 0;
        b = 2 * block;
        while b <= n {
            sym_merge(v, a, a + block, b, &less);
            a = b;
            b += 2 * block;
        }
        let m = a + block;
        if m < n {
            sym_merge(v, a, m, n, &less);
        }
        block *= 2;
    }
}

fn insertion_sort<T>(v: &mut [T], a: usize, b: usize, less: &impl Fn(&T, &T) -> bool) {
    for i in a + 1..b {
        let mut j = i;
        while j > a && less(&v[j], &v[j - 1]) {
            v.swap(j, j - 1);
            j -= 1;
        }
    }
}

fn sym_merge<T>(v: &mut [T], a: usize, m: usize, b: usize, less: &impl Fn(&T, &T) -> bool) {
    if m - a == 1 {
        let (mut i, mut j) = (m, b);
        while i < j {
            let h = (i + j) / 2;
            if less(&v[h], &v[a]) {
                i = h + 1;
            } else {
                j = h;
            }
        }
        for k in a..i.saturating_sub(1) {
            v.swap(k, k + 1);
        }
        return;
    }
    if b - m == 1 {
        let (mut i, mut j) = (a, m);
        while i < j {
            let h = (i + j) / 2;
            if !less(&v[m], &v[h]) {
                i = h + 1;
            } else {
                j = h;
            }
        }
        let mut k = m;
        while k > i {
            v.swap(k, k - 1);
            k -= 1;
        }
        return;
    }
    let mid = (a + b) / 2;
    let n = mid + m;
    let (mut start, mut r) = if m > mid { (n - b, mid) } else { (a, m) };
    let p = n - 1;
    while start < r {
        let c = (start + r) / 2;
        if !less(&v[p - c], &v[c]) {
            start = c + 1;
        } else {
            r = c;
        }
    }
    let end = n - start;
    if start < m && m < end {
        rotate(v, start, m, end);
    }
    if a < start && start < mid {
        sym_merge(v, a, start, mid, less);
    }
    if mid < end && end < b {
        sym_merge(v, mid, end, b, less);
    }
}

fn rotate<T>(v: &mut [T], a: usize, m: usize, b: usize) {
    let (mut i, mut j) = (m - a, b - m);
    while i != j {
        if i > j {
            swap_range(v, m - i, m, j);
            i -= j;
        } else {
            swap_range(v, m - i, m + j - i, i);
            j -= i;
        }
    }
    swap_range(v, m - i, m, i);
}

fn swap_range<T>(v: &mut [T], a: usize, b: usize, n: usize) {
    for i in 0..n {
        v.swap(a + i, b + i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_on_total_orders() {
        let mut v: Vec<(u32, usize)> = (0..137).map(|i| ((i * 7919) % 13, i as usize)).collect();
        let mut want = v.clone();
        want.sort_by_key(|x| x.0);
        slice_stable(&mut v, |a, b| a.0 < b.0);
        assert_eq!(v, want);
    }
}
