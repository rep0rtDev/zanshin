using System;
using System.Collections.Generic;
using System.Collections.Immutable;
using System.Linq;
using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;
using OpenFeature;
using OpenFeature.Constant;
using OpenFeature.Model;
using Zanshin.Server;
using Reason = OpenFeature.Constant.Reason;

namespace Zanshin.OpenFeature
{
    /// <summary>
    /// OpenFeature provider over <see cref="ZanshinClient"/>. The evaluation context's targeting key is the
    /// player's external id (required); other context attributes are ignored for remote evaluation because
    /// the server evaluates against the identity's stored properties — update them with
    /// <see cref="ZanshinClient.UpdatePropertiesAsync"/>.
    /// </summary>
    public sealed class ZanshinProvider : FeatureProvider
    {
        private static readonly Metadata Meta = new Metadata("zanshin");
        private readonly ZanshinClient _client;
        private readonly TimeSpan _timeout;

        public ZanshinProvider(ZanshinClient client, TimeSpan? timeout = null)
        {
            _client = client ?? throw new ArgumentNullException(nameof(client));
            _timeout = timeout ?? TimeSpan.FromSeconds(3);
        }

        public override Metadata GetMetadata() => Meta;

        public override Task<ResolutionDetails<bool>> ResolveBooleanValueAsync(string flagKey, bool defaultValue, EvaluationContext? context = null, CancellationToken cancellationToken = default) =>
            ResolveAsync(flagKey, defaultValue, context, "bool", v => v.ValueKind == JsonValueKind.True ? true : v.ValueKind == JsonValueKind.False ? false : (bool?)null, cancellationToken);

        public override Task<ResolutionDetails<string>> ResolveStringValueAsync(string flagKey, string defaultValue, EvaluationContext? context = null, CancellationToken cancellationToken = default) =>
            ResolveAsync(flagKey, defaultValue, context, "string", v => v.ValueKind == JsonValueKind.String ? v.GetString() : null, cancellationToken);

        public override Task<ResolutionDetails<int>> ResolveIntegerValueAsync(string flagKey, int defaultValue, EvaluationContext? context = null, CancellationToken cancellationToken = default) =>
            ResolveAsync(flagKey, defaultValue, context, "int", v => v.ValueKind == JsonValueKind.Number && v.TryGetInt32(out var i) ? i : (int?)null, cancellationToken);

        public override Task<ResolutionDetails<double>> ResolveDoubleValueAsync(string flagKey, double defaultValue, EvaluationContext? context = null, CancellationToken cancellationToken = default) =>
            ResolveAsync(flagKey, defaultValue, context, "double", v => v.ValueKind == JsonValueKind.Number ? v.GetDouble() : (double?)null, cancellationToken);

        public override Task<ResolutionDetails<Value>> ResolveStructureValueAsync(string flagKey, Value defaultValue, EvaluationContext? context = null, CancellationToken cancellationToken = default) =>
            ResolveAsync(flagKey, defaultValue, context, "structure", v => v.ValueKind == JsonValueKind.Undefined || v.ValueKind == JsonValueKind.Null ? null : ToValue(v), cancellationToken);

        private async Task<ResolutionDetails<T>> ResolveAsync<T>(string flagKey, T defaultValue, EvaluationContext? context, string want,
            Func<JsonElement, object?> convert, CancellationToken ct)
        {
            var subject = context?.TargetingKey;
            if (string.IsNullOrEmpty(subject))
                return new ResolutionDetails<T>(flagKey, defaultValue, ErrorType.TargetingKeyMissing, Reason.Error, null, "targetingKey (player external id) is required");

            Flag flag;
            using (var cts = CancellationTokenSource.CreateLinkedTokenSource(ct))
            {
                if (_timeout > TimeSpan.Zero) cts.CancelAfter(_timeout);
                try
                {
                    flag = await _client.FlagAsync(subject!, flagKey, cts.Token).ConfigureAwait(false);
                }
                catch (ZanshinException e) when (e.Status == 404)
                {
                    return new ResolutionDetails<T>(flagKey, defaultValue, ErrorType.FlagNotFound, Reason.Error, null, e.Message);
                }
                catch (Exception e) when (!(e is OperationCanceledException && ct.IsCancellationRequested))
                {
                    return new ResolutionDetails<T>(flagKey, defaultValue, ErrorType.General, Reason.Error, null, e.Message);
                }
            }

            var meta = new Dictionary<string, object> { ["version"] = (int)flag.Version };
            if (!string.IsNullOrEmpty(flag.Reason.RuleId)) meta["rule_id"] = flag.Reason.RuleId!;
            if (!string.IsNullOrEmpty(flag.Reason.Experiment)) meta["experiment"] = flag.Reason.Experiment!;
            var flagMeta = new ImmutableMetadata(meta);

            var converted = convert(flag.Value);
            if (converted is T typed)
                return new ResolutionDetails<T>(flagKey, typed, ErrorType.None, MapReason(flag.Reason.Kind), flag.Variant, null, flagMeta);
            return new ResolutionDetails<T>(flagKey, defaultValue, ErrorType.TypeMismatch, Reason.Error, flag.Variant, "flag value is not a " + want, flagMeta);
        }

        internal static string MapReason(string kind)
        {
            switch (kind)
            {
                case "default":
                case "prerequisite":
                    return Reason.Default;
                case "disabled":
                    return Reason.Disabled;
                case "variant":
                case "experiment":
                    return Reason.Split;
                case "rule":
                case "override":
                case "live_event":
                    return Reason.TargetingMatch;
                default:
                    return Reason.Unknown;
            }
        }

        internal static Value ToValue(JsonElement e)
        {
            switch (e.ValueKind)
            {
                case JsonValueKind.True: return new Value(true);
                case JsonValueKind.False: return new Value(false);
                case JsonValueKind.Number: return new Value(e.GetDouble());
                case JsonValueKind.String: return new Value(e.GetString() ?? "");
                case JsonValueKind.Array: return new Value(e.EnumerateArray().Select(ToValue).ToList());
                case JsonValueKind.Object:
                    var b = Structure.Builder();
                    foreach (var p in e.EnumerateObject()) b.Set(p.Name, ToValue(p.Value));
                    return new Value(b.Build());
                default: return new Value();
            }
        }
    }
}
