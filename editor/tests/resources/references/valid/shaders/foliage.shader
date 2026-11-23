shader "ForestShader" {
    pass "Forward" {
        face "back"
        queue "opaque"
        hlsl {
            struct VertexInput { float3 position : POSITION; };
            float4 vs_main(VertexInput input) : SV_Position { return float4(input.position, 1); }
            float4 ps_main() : SV_Target { return float4(0, 1, 0, 1); }
        }
    }
}
