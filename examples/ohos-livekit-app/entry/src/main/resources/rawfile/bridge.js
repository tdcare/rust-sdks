// bridge.js — 注入到 WebView onPageEnd，监听 localStorage 中 Token 变化
// 并通过 hook localStorage.setItem 提前截获登录，阻止管理页闪现

(function() {
    'use strict';

    var notified = false; // 防止重复通知

    function notifyLogin(token, expiresAt, username, displayName) {
        if (notified) return;
        notified = true;
        // 立即跳转到空白页，阻止 Vue Router 渲染 /apps 管理页
        window.location.replace('about:blank');
        if (window.AndroidBridge) {
            window.AndroidBridge.onLoginSuccess(token, expiresAt, username, displayName);
        }
    }

    // ── 方案1: Hook localStorage.setItem 在 Vue Router 导航前截获 ──
    (function() {
        var _setItem = Storage.prototype.setItem;
        Storage.prototype.setItem = function(key, value) {
            _setItem.apply(this, arguments);
            if (key === 'admin_token' && value) {
                try {
                    var expiresAt = this.getItem('admin_token_expires_at') || '0';
                    var userJson = this.getItem('admin_user');
                    var user = {};
                    try { user = userJson ? JSON.parse(userJson) : {}; } catch(e) {}
                    notifyLogin(value, expiresAt, user.username || '', user.display_name || '');
                } catch(e) {}
            }
        };
    })();

    // ── 方案2: 轮询检测（兜底，处理 bridge.js 加载时 token 已存在的场景）──
    var lastToken = null;
    setInterval(function() {
        try {
            var token = localStorage.getItem('admin_token');
            if (token && token !== lastToken) {
                lastToken = token;
                var expiresAt = localStorage.getItem('admin_token_expires_at') || '0';
                var userJson = localStorage.getItem('admin_user');
                var user = {};
                try { user = userJson ? JSON.parse(userJson) : {}; } catch(e) {}
                notifyLogin(token, expiresAt, user.username || '', user.display_name || '');
            }
        } catch (e) {
            // 忽略轮询异常
        }
    }, 500);

    // ── 拦截 XMLHttpRequest 以检测 /refresh 接口失败 ──
    var origOpen = XMLHttpRequest.prototype.open;
    XMLHttpRequest.prototype.open = function(method, url) {
        this._auth_url = url;
        return origOpen.apply(this, arguments);
    };

    var origSend = XMLHttpRequest.prototype.send;
    XMLHttpRequest.prototype.send = function() {
        var self = this;
        var origOnReady = this.onreadystatechange;
        this.onreadystatechange = function() {
            try {
                if (self.readyState === 4 && self._auth_url &&
                    self._auth_url.indexOf('/refresh') !== -1) {
                    var resp = JSON.parse(self.responseText);
                    if (resp.code !== '0' && resp.code !== 'SUCCESS' && resp.code !== '200') {
                        if (window.AndroidBridge) {
                            window.AndroidBridge.onLogout('token_refresh_failed');
                        }
                    }
                }
            } catch (e) {}
            if (origOnReady) origOnReady.apply(this, arguments);
        };
        return origSend.apply(this, arguments);
    };

    // ── 拦截 fetch 以检测 /refresh 失败 ──
    var origFetch = window.fetch;
    window.fetch = function(input, init) {
        var url = typeof input === 'string' ? input : (input.url || '');
        return origFetch.apply(this, arguments).then(function(response) {
            if (url.indexOf('/refresh') !== -1 && !response.ok) {
                if (window.AndroidBridge) {
                    window.AndroidBridge.onLogout('token_refresh_failed');
                }
            }
            return response;
        });
    };
})();
