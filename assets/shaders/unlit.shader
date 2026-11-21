shader "Unlit" {
    pass "Forward" {
        face = back;
        queue = opaque;
        hlsl {
            struct VertexInput
            {
                float3 position : POSITION;
                float4 color : COLOR0;
            };

            struct InstanceInput
            {
                float4 world0 : INSTANCE0;
                float4 world1 : INSTANCE1;
                float4 world2 : INSTANCE2;
                float4 world3 : INSTANCE3;
            };

            struct VertexOutput
            {
                float4 position : SV_Position;
                float4 color : COLOR0;
            };

            VertexOutput VSMain(VertexInput input, InstanceInput instance)
            {
                VertexOutput output;
                float4 position = float4(input.position, 1.0);
                output.position = position.x * instance.world0
                    + position.y * instance.world1
                    + position.z * instance.world2
                    + position.w * instance.world3;
                output.color = input.color;
                return output;
            }

            float4 PSMain(VertexOutput input) : SV_Target0
            {
                return input.color;
            }
        }
    }

    pass "ShadowCaster" {
        face = front;
        queue = opaque;
        hlsl {
            struct VertexInput
            {
                float3 position : POSITION;
            };

            float4 VSMain(VertexInput input) : SV_Position
            {
                return float4(input.position, 1.0);
            }

            float4 PSMain() : SV_Target0
            {
                return float4(0.0, 0.0, 0.0, 1.0);
            }
        }
    }
}
