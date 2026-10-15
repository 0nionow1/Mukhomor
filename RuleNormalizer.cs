// Pure normalization for large rule lists. No file, network or process access.
using System;
using System.Globalization;
using System.Net;
using System.Net.Sockets;
using System.Text.RegularExpressions;

public static class MukhomorRuleNormalizer {
    static readonly IdnMapping Idn = new IdnMapping();
    static readonly Regex Control = new Regex("[\\x00-\\x1f\\x7f]", RegexOptions.CultureInvariant);
    static readonly Regex ProcessName = new Regex(@"^[^\\/:,*?\r\n]+\.exe$", RegexOptions.IgnoreCase | RegexOptions.CultureInvariant);
    static readonly Regex ProcessPath = new Regex(@"^[A-Za-z]:\\.+\.exe$", RegexOptions.IgnoreCase | RegexOptions.CultureInvariant);
    static readonly Regex PathInvalid = new Regex(@"[,\r\n*?]", RegexOptions.CultureInvariant);
    static readonly Regex DomainInvalid = new Regex(@"[/:,\s*+]", RegexOptions.CultureInvariant);
    static readonly Regex Label = new Regex(@"^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$", RegexOptions.IgnoreCase | RegexOptions.CultureInvariant);
    static readonly Regex V4 = new Regex(@"^\d+\.\d+\.\d+\.\d+$", RegexOptions.CultureInvariant);
    static readonly Regex Digits = new Regex(@"^\d+$", RegexOptions.CultureInvariant);

    public static string Convert(string section, string input) {
        string value = input.Trim();
        if (value.Length == 0 || Control.IsMatch(value)) throw new ArgumentException("Empty rule or control character");
        switch (section) {
            case "process_names":
                if (!ProcessName.IsMatch(value)) throw new ArgumentException("Expected an EXE name without a path");
                return value;
            case "process_paths":
                if (PathInvalid.IsMatch(value) || !ProcessPath.IsMatch(value)) throw new ArgumentException("Expected an absolute EXE path");
                return value;
            case "domains": return Domain(value, false);
            case "domain_suffixes":
            case "rule_set_exclusions": return Domain(value, true);
            case "ip_cidrs": return Cidr(value);
            default: throw new ArgumentException("Unknown rule section");
        }
    }
    static string Domain(string input, bool suffix) {
        string value = input.Trim().TrimEnd('.').ToLowerInvariant();
        if (suffix) {
            if (value.StartsWith("*.") || value.StartsWith("+.")) value = value.Substring(2);
            else if (value.StartsWith(".")) value = value.Substring(1);
        }
        if (value.Length == 0 || value.Length > 253 || DomainInvalid.IsMatch(value)) throw new ArgumentException("Invalid domain: " + input);
        try { value = Idn.GetAscii(value); } catch { throw new ArgumentException("Invalid domain: " + input); }
        IPAddress address;
        if (IPAddress.TryParse(value, out address)) throw new ArgumentException("Use the IP list for IP addresses: " + input);
        foreach (string label in value.Split('.'))
            if (label.Length > 63 || !Label.IsMatch(label)) throw new ArgumentException("Invalid domain: " + input);
        if (!suffix && !value.Contains(".")) throw new ArgumentException("Expected a full domain: " + input);
        return value;
    }
    static string Cidr(string input) {
        string[] parts = input.Trim().Split('/');
        if (parts.Length > 2) throw new ArgumentException("Invalid IP/CIDR: " + input);
        IPAddress address;
        if (!IPAddress.TryParse(parts[0], out address) || parts[0].Contains("%") ||
            (address.AddressFamily == AddressFamily.InterNetwork && !V4.IsMatch(parts[0]))) throw new ArgumentException("Invalid IP: " + input);
        int width = address.AddressFamily == AddressFamily.InterNetwork ? 32 : 128;
        int prefix = width;
        if (parts.Length == 2 && (!Digits.IsMatch(parts[1]) || !Int32.TryParse(parts[1], out prefix))) throw new ArgumentException("Invalid prefix: " + input);
        if (prefix < 1 || prefix > width) throw new ArgumentException("Prefix would bypass all traffic or is invalid: " + input);
        byte[] bytes = address.GetAddressBytes();
        for (int i = 0; i < bytes.Length; i++) {
            int bits = Math.Max(0, Math.Min(8, prefix - 8*i));
            bytes[i] = (byte)(bytes[i] & (bits == 0 ? 0 : (255 << (8-bits)) & 255));
        }
        return new IPAddress(bytes).ToString() + "/" + prefix;
    }
    public static string[] SortCidrs(string[] input) {
        string[] values = (string[])input.Clone();
        string[] keys = new string[values.Length];
        for (int i = 0; i < values.Length; i++) {
            string[] parts = values[i].Split('/');
            byte[] bytes = IPAddress.Parse(parts[0]).GetAddressBytes();
            string[] address = new string[bytes.Length];
            for (int j = 0; j < bytes.Length; j++) address[j] = bytes[j].ToString("D3", CultureInfo.InvariantCulture);
            keys[i] = bytes.Length.ToString("D2", CultureInfo.InvariantCulture) + ":" + String.Join(".", address) + "/" + Int32.Parse(parts[1], CultureInfo.InvariantCulture).ToString("D3", CultureInfo.InvariantCulture);
        }
        Array.Sort(keys, values, StringComparer.Ordinal);
        return values;
    }
}
