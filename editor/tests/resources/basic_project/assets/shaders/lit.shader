shader "LitShader" {
    pass "Main" {
        face "back"
        queue "opaque"
        hlsl {
            #include "common/vertex.hlsl"

            float4 VSMain(VertexInput input) : SV_POSITION {
                return float4(input.position, 1.0);
            }

            float4 PSMain() : SV_TARGET {
                return float4(1.0, 1.0, 1.0, 1.0);
            }
        }
    }
}
