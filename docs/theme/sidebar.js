// Start with the sidebar closed unless the user has already set a preference.
if (!localStorage.getItem('mdbook-sidebar')) {
    localStorage.setItem('mdbook-sidebar', 'hidden');
}
