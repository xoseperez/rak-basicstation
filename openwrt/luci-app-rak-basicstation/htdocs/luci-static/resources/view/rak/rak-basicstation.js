'use strict';
'require form';
'require uci';
'require view';

return view.extend({
    render: function() {
        var m, s, o;

        m = new form.Map('rak-basicstation',
            _('RAK BasicStation Forwarder'),
            _('Configuration for the LoRa Basics Station protocol forwarder.'));

        m.tabbed = true;

        /* ================================================================
         * Tab: General
         * ================================================================ */
        s = m.section(form.NamedSection, 'global', 'global', _('General'));
        s.addremove = false;

        o = s.option(form.Flag, 'enabled', _('Enable'));
        o.rmempty = false;

        /* ================================================================
         * Tab: Backend
         * ================================================================ */
        s = m.section(form.NamedSection, 'backend', 'backend', _('Backend'));
        s.addremove = false;

        o = s.option(form.Value, 'gateway_id', _('Gateway ID'),
            _('Override gateway EUI (e.g. 0102030405060708). Leave empty to use the ID reported by the concentratord.'));
        o.optional = true;
        o.placeholder = '';

        o = s.option(form.ListValue, 'concentratord_event_url', _('Concentratord slot'));
        o.value('ipc:///tmp/concentratord_event',       _('Single Slot Gateway'));
        o.value('ipc:///tmp/concentratord_slot1_event', _('Slot 1'));
        o.value('ipc:///tmp/concentratord_slot2_event', _('Slot 2'));
        o.value('ipc:///tmp/gateway_relay_event',       _('Gateway Mesh'));
        o.rmempty = false;
        o.onchange = function(target, section_id, value) {
            uci.set('rak-basicstation', section_id, 'concentratord_command_url',
                value.replace('_event', '_command'));
        };

        o = s.option(form.Flag, 'concentratord_context_caching',
            _('Context caching'),
            _('Cache the raw context blob from uplinks to restore it on downlinks. Enable for Gateway Mesh compatibility.'));
        o.rmempty = false;

        /* ================================================================
         * Tab: LNS
         * ================================================================ */
        s = m.section(form.NamedSection, 'lns', 'lns', _('LNS'));
        s.addremove = false;

        o = s.option(form.Value, 'server', _('LNS server'),
            _('WebSocket URI of the LNS (e.g. wss://lns.example.com:8887).'));
        o.placeholder = 'wss://localhost:8887';
        o.rmempty = false;

        o = s.option(form.TextValue, 'ca_cert_content',
            _('CA certificate (PEM)'),
            _('Paste the CA certificate in PEM format. Leave empty for no CA verification override. For TTN/TTI you can use the certificate <a href="https://www.thethingsindustries.com/docs/concepts/advanced/root-certificates/ca.pem">here</a>, other LNS provide the CA certificate in the gateway page.'));
        o.rows = 10;
        o.optional = true;

        o = s.option(form.TextValue, 'tls_cert_content',
            _('Client certificate (PEM)'),
            _('Paste the client TLS certificate in PEM format. Leave empty if not using mutual TLS.'));
        o.rows = 10;
        o.optional = true;

        o = s.option(form.TextValue, 'tls_key_content',
            _('Client key / auth token (PEM)'),
            _('Paste the client private key in PEM format or the auth token.'));
        o.rows = 10;
        o.optional = true;

        /* ================================================================
         * Tab: CUPS
         * ================================================================ */
        s = m.section(form.NamedSection, 'cups', 'cups', _('CUPS'));
        s.addremove = false;

        o = s.option(form.Flag, 'enabled', _('Enable CUPS'));
        o.rmempty = false;

        o = s.option(form.Value, 'server', _('CUPS server'),
            _('HTTPS URI of the CUPS server (e.g. https://cups.example.com:443).'));
        o.optional = true;
        o.placeholder = '';

        o = s.option(form.TextValue, 'ca_cert_content',
            _('CA certificate (PEM)'),
            _('Paste the CA certificate in PEM format. Leave empty for no CA verification override. For TTN/TTI you can use the certificate <a href="https://www.thethingsindustries.com/docs/concepts/advanced/root-certificates/ca.pem">here</a>, other LNS provide the CA certificate in the gateway page.'));
        o.rows = 10;
        o.optional = true;

        o = s.option(form.TextValue, 'tls_cert_content',
            _('Client certificate (PEM)'),
            _('Paste the client TLS certificate in PEM format. Leave empty if not using mutual TLS.'));
        o.rows = 10;
        o.optional = true;

        o = s.option(form.TextValue, 'tls_key_content',
            _('Client key (PEM)'),
            _('Paste the client private key in PEM format or the auth token.'));
        o.rows = 10;
        o.optional = true;

        return m.render();
    }
});
